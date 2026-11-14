//! Agenda via iCal (ex.: "endereço secreto no formato iCal" do Google Agenda).
//! Expande eventos recorrentes (RRULE/EXDATE/RECURRENCE-ID) e dispara avisos piscando.

use std::collections::HashSet;
use std::thread;
use std::time::{Duration, Instant};

use chrono::{DateTime, Local, NaiveDate, NaiveDateTime, TimeZone, Utc};
use rrule::RRuleSet;

use crate::config::{AgendaCfg, Effect};
use crate::state::{Event, Message, Rgb, Shared, Wake};

struct Prop {
    name: String,
    params: Vec<(String, String)>,
    value: String,
}

impl Prop {
    fn param(&self, key: &str) -> Option<&str> {
        self.params.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    /// Reconstrói a linha original (para o parser de RRULE).
    fn raw(&self) -> String {
        let mut s = self.name.clone();
        for (k, v) in &self.params {
            s.push_str(&format!(";{k}={v}"));
        }
        format!("{s}:{}", self.value)
    }
}

fn parse_prop(line: &str) -> Option<Prop> {
    // Separa no primeiro ':' que não esteja entre aspas.
    let mut in_quotes = false;
    let split = line.char_indices().find(|&(_, c)| {
        if c == '"' {
            in_quotes = !in_quotes;
        }
        c == ':' && !in_quotes
    })?.0;
    let (head, value) = (&line[..split], &line[split + 1..]);
    let mut parts = head.split(';');
    let name = parts.next()?.to_uppercase();
    let params = parts
        .filter_map(|p| p.split_once('='))
        .map(|(k, v)| (k.to_uppercase(), v.trim_matches('"').to_string()))
        .collect();
    Some(Prop { name, params, value: value.to_string() })
}

fn unescape(s: &str) -> String {
    s.replace("\\n", " ").replace("\\N", " ").replace("\\,", ",").replace("\\;", ";").replace("\\\\", "\\")
}

/// Converte DTSTART/DTEND em horário local. Retorna (instante, é_dia_inteiro).
fn parse_dt(p: &Prop) -> Option<(DateTime<Local>, bool)> {
    let v = p.value.trim();
    if p.param("VALUE") == Some("DATE") || v.len() == 8 {
        let d = NaiveDate::parse_from_str(v, "%Y%m%d").ok()?;
        let dt = Local.from_local_datetime(&d.and_hms_opt(0, 0, 0)?).earliest()?;
        return Some((dt, true));
    }
    if let Some(utc) = v.strip_suffix('Z') {
        let n = NaiveDateTime::parse_from_str(utc, "%Y%m%dT%H%M%S").ok()?;
        return Some((Utc.from_utc_datetime(&n).with_timezone(&Local), false));
    }
    let n = NaiveDateTime::parse_from_str(v, "%Y%m%dT%H%M%S").ok()?;
    let tz = p.param("TZID").and_then(|t| t.parse::<chrono_tz::Tz>().ok());
    let dt = match tz {
        Some(tz) => tz.from_local_datetime(&n).earliest()?.with_timezone(&Local),
        None => Local.from_local_datetime(&n).earliest()?,
    };
    Some((dt, false))
}

/// Linha DTSTART/EXDATE num formato que o crate `rrule` entende.
fn rrule_line(p: &Prop, start: DateTime<Local>, all_day: bool) -> String {
    let tz_ok = p.param("TZID").is_none_or(|t| t.parse::<chrono_tz::Tz>().is_ok());
    if all_day || !tz_ok {
        // Datas puras e fusos fora do padrão IANA viram UTC.
        format!("{}:{}", p.name, start.with_timezone(&Utc).format("%Y%m%dT%H%M%SZ"))
    } else {
        p.raw()
    }
}

struct RawEvent {
    props: Vec<Prop>,
}

impl RawEvent {
    fn get(&self, name: &str) -> Option<&Prop> {
        self.props.iter().find(|p| p.name == name)
    }
}

fn parse_ics(text: &str) -> Vec<RawEvent> {
    // "Desdobra" linhas longas (continuação começa com espaço ou tab).
    let mut lines: Vec<String> = Vec::new();
    for l in text.lines() {
        if (l.starts_with(' ') || l.starts_with('\t')) && !lines.is_empty() {
            lines.last_mut().unwrap().push_str(&l[1..]);
        } else {
            lines.push(l.to_string());
        }
    }
    let mut events = Vec::new();
    let mut current: Option<Vec<Prop>> = None;
    let mut depth = 0; // ignora VALARM e outros componentes aninhados
    for l in &lines {
        match l.trim_end() {
            "BEGIN:VEVENT" => {
                current = Some(Vec::new());
                depth = 0;
            }
            "END:VEVENT" => {
                if let Some(props) = current.take() {
                    events.push(RawEvent { props });
                }
            }
            l if l.starts_with("BEGIN:") && current.is_some() => depth += 1,
            l if l.starts_with("END:") && current.is_some() => depth -= 1,
            l => {
                if let (Some(props), 0) = (current.as_mut(), depth) {
                    if let Some(p) = parse_prop(l) {
                        props.push(p);
                    }
                }
            }
        }
    }
    events
}

fn expand(raw: Vec<RawEvent>, from: DateTime<Local>, to: DateTime<Local>) -> Vec<Event> {
    let mut out = Vec::new();
    let mut overridden: HashSet<(String, i64)> = HashSet::new();
    let mut series = Vec::new();

    for ev in &raw {
        if ev.get("STATUS").is_some_and(|s| s.value.eq_ignore_ascii_case("CANCELLED")) {
            // Ocorrência cancelada de uma série: também precisa ser excluída.
            if let (Some(uid), Some(rid)) = (ev.get("UID"), ev.get("RECURRENCE-ID").and_then(parse_dt)) {
                overridden.insert((uid.value.clone(), rid.0.timestamp()));
            }
            continue;
        }
        let Some(dtstart) = ev.get("DTSTART") else { continue };
        let Some((start, all_day)) = parse_dt(dtstart) else { continue };
        let end = ev
            .get("DTEND")
            .and_then(parse_dt)
            .map(|e| e.0)
            .unwrap_or(start + if all_day { chrono::Duration::days(1) } else { chrono::Duration::hours(1) });
        let title = ev.get("SUMMARY").map(|s| unescape(&s.value)).unwrap_or_else(|| "(sem título)".into());
        let uid = ev.get("UID").map(|u| u.value.clone()).unwrap_or_default();

        if let Some(rid) = ev.get("RECURRENCE-ID").and_then(parse_dt) {
            overridden.insert((uid, rid.0.timestamp()));
            out.push(Event { title, start, end, all_day });
        } else if let Some(rrule) = ev.get("RRULE") {
            let mut spec = format!("{}\nRRULE:{}", rrule_line(dtstart, start, all_day), rrule.value);
            let exdates: Vec<String> = ev
                .props
                .iter()
                .filter(|p| p.name == "EXDATE")
                .flat_map(|p| {
                    p.value.split(',').filter_map(|v| {
                        let single = Prop { name: "EXDATE".into(), params: p.params.clone(), value: v.into() };
                        let (dt, ad) = parse_dt(&single)?;
                        Some(rrule_line(&single, dt, ad))
                    }).collect::<Vec<_>>()
                })
                .collect();
            let with_ex = format!("{spec}\n{}", exdates.join("\n"));
            let set = with_ex.trim().parse::<RRuleSet>().or_else(|_| {
                spec = spec.trim().to_string();
                spec.parse::<RRuleSet>()
            });
            if let Ok(set) = set {
                series.push((uid, title, end - start, all_day, set));
            }
        } else {
            out.push(Event { title, start, end, all_day });
        }
    }

    for (uid, title, dur, all_day, set) in series {
        let after = (from - dur - chrono::Duration::days(1)).with_timezone(&rrule::Tz::UTC);
        let before = to.with_timezone(&rrule::Tz::UTC);
        for occ in set.after(after).before(before).all(200).dates {
            let start = occ.with_timezone(&Local);
            if overridden.contains(&(uid.clone(), start.timestamp())) {
                continue;
            }
            out.push(Event { title: title.clone(), start, end: start + dur, all_day });
        }
    }

    out.retain(|e| e.end > from && e.start < to);
    out.sort_by_key(|e| e.start);
    out
}

fn fetch(url: &str) -> Result<String, String> {
    let url = url.trim();
    if url.starts_with("http://") || url.starts_with("https://") || url.starts_with("webcal://") {
        let url = url.replacen("webcal://", "https://", 1);
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(20)))
            .build()
            .into();
        agent
            .get(&url)
            .call()
            .map_err(|e| e.to_string())?
            .body_mut()
            .with_config()
            .limit(20 * 1024 * 1024)
            .read_to_string()
            .map_err(|e| e.to_string())
    } else {
        // Também aceita um arquivo .ics local (útil para testes).
        std::fs::read_to_string(url).map_err(|e| e.to_string())
    }
}

fn refresh(cfg: &AgendaCfg) -> Result<Vec<Event>, String> {
    let now = Local::now();
    let to = now + chrono::Duration::hours(48);
    let mut all = Vec::new();
    for url in &cfg.urls {
        let text = fetch(url)?;
        all.extend(expand(parse_ics(&text), now, to));
    }
    all.sort_by_key(|e| e.start);
    Ok(all)
}

pub fn spawn(state: Shared, wake: Wake) {
    thread::spawn(move || {
        let mut last_cfg: Option<AgendaCfg> = None;
        let mut last_fetch = Instant::now();
        let mut notified: HashSet<(String, i64, i64)> = HashSet::new();
        loop {
            let cfg = state.lock().unwrap().agenda_cfg.clone();
            let interval = Duration::from_secs(cfg.atualizar_minutos.max(1) * 60);
            if last_cfg.as_ref() != Some(&cfg) || last_fetch.elapsed() >= interval {
                let result = if cfg.urls.is_empty() { Ok(vec![]) } else { refresh(&cfg) };
                let mut s = state.lock().unwrap();
                match result {
                    Ok(events) => {
                        s.events = events;
                        s.agenda_error = None;
                    }
                    Err(e) => s.agenda_error = Some(e),
                }
                drop(s);
                last_cfg = Some(cfg.clone());
                last_fetch = Instant::now();
                wake();
            }

            // Avisos: dispara quando entra na janela [início - N min, +2 min).
            let now = Local::now();
            let mut alerts = Vec::new();
            {
                let s = state.lock().unwrap();
                for e in s.events.iter().filter(|e| !e.all_day) {
                    for &m in &cfg.avisar_minutos {
                        let trigger = e.start - chrono::Duration::minutes(m);
                        let key = (e.title.clone(), e.start.timestamp(), m);
                        if now >= trigger && now < trigger + chrono::Duration::minutes(2) && notified.insert(key) {
                            let (text, color) = if m > 0 {
                                (format!("EM {m} MIN: {}", e.title), Rgb::ORANGE)
                            } else {
                                (format!("AGORA: {}", e.title), Rgb::RED)
                            };
                            alerts.push(Message { text, color, effect: Effect::Piscar, repeat: 3, seconds: 15.0 });
                        }
                    }
                }
            }
            if !alerts.is_empty() {
                let mut s = state.lock().unwrap();
                for a in alerts {
                    s.push_message(a);
                }
                drop(s);
                wake();
            }
            let cutoff = now.timestamp() - 86_400;
            notified.retain(|k| k.1 > cutoff);

            thread::sleep(Duration::from_secs(5));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_recurring_and_overrides() {
        let today = Local::now().date_naive();
        let d = today.format("%Y%m%d");
        let ics = format!(
            "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:a\r\nDTSTART;TZID=America/Sao_Paulo:{d}T090000\r\n\
             DTEND;TZID=America/Sao_Paulo:{d}T100000\r\nRRULE:FREQ=DAILY;COUNT=5\r\nSUMMARY:Daily\\, time\r\n\
             BEGIN:VALARM\r\nSUMMARY:ignorar\r\nEND:VALARM\r\nEND:VEVENT\r\n\
             BEGIN:VEVENT\r\nUID:b\r\nDTSTART;VALUE=DATE:{d}\r\nSUMMARY:Feriado\r\nEND:VEVENT\r\n\
             END:VCALENDAR\r\n"
        );
        let from = Local.from_local_datetime(&today.and_hms_opt(0, 0, 0).unwrap()).unwrap();
        let ev = expand(parse_ics(&ics), from, from + chrono::Duration::hours(48));
        let titles: Vec<_> = ev.iter().map(|e| e.title.as_str()).collect();
        assert_eq!(titles.iter().filter(|t| **t == "Daily, time").count(), 2, "{titles:?}");
        assert!(titles.contains(&"Feriado"));
    }
}
