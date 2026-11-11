//! A run's numbers as a markdown table and as JSON.

use serde_json::{Map, Value, json};

use super::stats::{Settle, Summary};

/// One measured path: what the table shows and what the JSON holds.
pub struct Row {
    pub path: String,
    pub shown: String,
    pub data: Value,
    pub note: String,
}

impl Row {
    pub fn duration(path: &str, ms: f64) -> Self {
        Self {
            path: path.to_string(),
            shown: duration(ms),
            data: json!({ "ms": round(ms) }),
            note: String::new(),
        }
    }

    pub fn summary(path: &str, summary: &Summary) -> Self {
        Self {
            path: path.to_string(),
            shown: format!(
                "med {}, p95 {}, max {} ({})",
                duration(summary.median),
                duration(summary.p95),
                duration(summary.max),
                summary.count
            ),
            data: json!({
                "count": summary.count,
                "median_ms": round(summary.median),
                "p95_ms": round(summary.p95),
                "max_ms": round(summary.max),
            }),
            note: String::new(),
        }
    }

    pub fn settle(path: &str, settle: &Settle) -> Self {
        Self {
            path: path.to_string(),
            shown: format!(
                "call {}, settled {}, worst frame {}",
                duration(settle.call),
                duration(settle.settled),
                duration(settle.worst_frame)
            ),
            data: json!({
                "call_ms": round(settle.call),
                "settled_ms": round(settle.settled),
                "worst_frame_ms": round(settle.worst_frame),
            }),
            note: String::new(),
        }
    }

    pub fn bytes(path: &str, bytes: u64) -> Self {
        Self {
            path: path.to_string(),
            shown: self::bytes(bytes),
            data: json!({ "bytes": bytes }),
            note: String::new(),
        }
    }

    /// A path the run could not measure, and why.
    pub fn missing(path: &str, why: &str) -> Self {
        Self {
            path: path.to_string(),
            shown: "n/a".to_string(),
            data: Value::Null,
            note: why.to_string(),
        }
    }

    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.note = note.into();
        self
    }
}

/// Everything one run measured, and what it ran on.
pub struct Report {
    pub fields: Vec<(&'static str, Value)>,
    pub rows: Vec<Row>,
}

impl Report {
    pub fn to_markdown(&self) -> String {
        let mut out = String::new();
        for (name, value) in &self.fields {
            let value = match value {
                Value::String(text) => text.clone(),
                other => other.to_string(),
            };
            out.push_str(&format!("- {name}: {}\n", cell(&value)));
        }
        out.push_str("\n| Path | Result | Notes |\n|---|---|---|\n");
        for row in &self.rows {
            out.push_str(&format!(
                "| {} | {} | {} |\n",
                cell(&row.path),
                cell(&row.shown),
                cell(&row.note)
            ));
        }
        out
    }

    pub fn to_json(&self) -> Value {
        let mut out = Map::new();
        for (name, value) in &self.fields {
            out.insert((*name).to_string(), value.clone());
        }
        let rows: Vec<Value> = self
            .rows
            .iter()
            .map(|row| json!({ "path": row.path, "result": row.data, "note": row.note }))
            .collect();
        out.insert("rows".to_string(), Value::Array(rows));
        Value::Object(out)
    }
}

/// `ms` as milliseconds below a second and seconds above.
pub fn duration(ms: f64) -> String {
    if ms < 1000.0 {
        format!("{ms:.0} ms")
    } else {
        format!("{:.1} s", ms / 1000.0)
    }
}

pub fn bytes(bytes: u64) -> String {
    const KB: f64 = 1000.0;
    let value = bytes as f64;
    if value >= KB * KB * KB {
        format!("{:.2} GB", value / (KB * KB * KB))
    } else if value >= KB * KB {
        format!("{:.1} MB", value / (KB * KB))
    } else if value >= KB {
        format!("{:.1} KB", value / KB)
    } else {
        format!("{bytes} B")
    }
}

fn round(ms: f64) -> f64 {
    (ms * 10.0).round() / 10.0
}

/// Text safe inside a table cell.
fn cell(text: &str) -> String {
    text.replace('|', "\\|").replace('\n', " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_read_in_milliseconds_then_seconds() {
        assert_eq!(duration(16.64), "17 ms");
        assert_eq!(duration(999.4), "999 ms");
        assert_eq!(duration(12_140.0), "12.1 s");
    }

    #[test]
    fn sizes_use_decimal_units() {
        assert_eq!(bytes(829), "829 B");
        assert_eq!(bytes(15_700), "15.7 KB");
        assert_eq!(bytes(18_663_204), "18.7 MB");
        assert_eq!(bytes(2_040_000_000), "2.04 GB");
    }

    #[test]
    fn the_table_lists_fields_then_one_row_per_path() {
        let report = Report {
            fields: vec![("scene", json!("assets/a|b.bsn")), ("nodes", json!(817))],
            rows: vec![
                Row::duration("Launch to scene loaded", 12_100.0),
                Row::missing("Plain run, first frame", "needs a display hook"),
                Row::settle(
                    "Undo",
                    &Settle {
                        call: 4.0,
                        settled: 380.0,
                        worst_frame: 352.0,
                    },
                )
                .with_note("one step"),
            ],
        };
        assert_eq!(
            report.to_markdown(),
            "- scene: assets/a\\|b.bsn\n- nodes: 817\n\n\
             | Path | Result | Notes |\n|---|---|---|\n\
             | Launch to scene loaded | 12.1 s |  |\n\
             | Plain run, first frame | n/a | needs a display hook |\n\
             | Undo | call 4 ms, settled 380 ms, worst frame 352 ms | one step |\n"
        );
    }

    #[test]
    fn the_json_holds_fields_and_every_number() {
        let summary = Summary::of(&[16.0, 17.0, 40.0]).unwrap();
        let report = Report {
            fields: vec![("scene_bytes", json!(829))],
            rows: vec![
                Row::summary("Idle", &summary),
                Row::missing("Plain run", "why"),
            ],
        };
        let json = report.to_json();
        assert_eq!(json["scene_bytes"], 829);
        assert_eq!(json["rows"][0]["result"]["median_ms"], 17.0);
        assert_eq!(json["rows"][0]["result"]["max_ms"], 40.0);
        assert_eq!(json["rows"][1]["result"], Value::Null);
        assert_eq!(json["rows"][1]["note"], "why");
    }

    #[test]
    fn a_summary_row_reads_median_p95_and_max() {
        let summary = Summary::of(&[16.4, 16.8, 23.0]).unwrap();
        assert_eq!(
            Row::summary("Idle", &summary).shown,
            "med 17 ms, p95 23 ms, max 23 ms (3)"
        );
    }
}
