//! Flat-file JSON database — ports `bridge/index.js`'s terminals/transactions
//! store byte-for-byte (same on-disk shape: `{ terminals, transactions, ecrSeq }`).

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Terminal {
    pub id: String,
    pub name: String,
    pub model: String,
    pub conn_type: String, // 'tcp' | 'usb'
    pub ip: String,
    pub port: u16,
    pub serial_path: String,
    pub baud_rate: u32,
    pub created_at: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct DbData {
    #[serde(default)]
    pub terminals: Vec<Terminal>,
    #[serde(default)]
    pub transactions: Vec<Map<String, Value>>,
    #[serde(default, rename = "ecrSeq")]
    pub ecr_seq: HashMap<String, u32>,
}

pub struct Db {
    file_path: PathBuf,
    pid: u32,
    data: DbData,
}

fn json_number(v: Option<&Value>) -> Option<f64> {
    match v {
        Some(Value::Number(n)) => n.as_f64(),
        Some(Value::String(s)) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
}

fn json_str_trim(v: Option<&Value>) -> Option<String> {
    v.and_then(Value::as_str).map(|s| s.trim().to_string())
}

fn to_base36(mut n: i64) -> String {
    if n == 0 {
        return "0".to_string();
    }
    let digits = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let neg = n < 0;
    if neg {
        n = -n;
    }
    let mut out = Vec::new();
    while n > 0 {
        out.push(digits[(n % 36) as usize]);
        n /= 36;
    }
    if neg {
        out.push(b'-');
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_default()
}

fn rand_base36(len: usize) -> String {
    (0..len)
        .map(|_| {
            let idx = rand::random::<u32>() % 36;
            std::char::from_digit(idx, 36).unwrap_or('0')
        })
        .collect()
}

pub struct TxQuery {
    pub status: Option<String>,
    pub tx_type: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub terminal_id: Option<String>,
    pub page: Option<i64>,
    pub page_size: Option<i64>,
}

pub struct TxQueryResult {
    pub total: usize,
    pub page: i64,
    pub page_size: i64,
    pub page_count: i64,
    pub rows: Vec<Map<String, Value>>,
}

impl Db {
    /// Generate an id like `<prefix>_<base36 millis><6 random base36 chars>`.
    pub fn gen_id(prefix: &str) -> String {
        let now_ms = chrono::Utc::now().timestamp_millis();
        format!("{}_{}{}", prefix, to_base36(now_ms), rand_base36(6))
    }

    /// Load from `<data_dir>/pax-db.json`, creating it with defaults if missing,
    /// or backing up + starting fresh if corrupt.
    pub fn load(data_dir: &std::path::Path) -> Self {
        let _ = std::fs::create_dir_all(data_dir);
        let file_path = data_dir.join("pax-db.json");
        let mut needs_persist = false;

        let data = if file_path.exists() {
            match std::fs::read_to_string(&file_path)
                .ok()
                .and_then(|c| serde_json::from_str::<DbData>(&c).ok())
            {
                Some(d) => d,
                None => {
                    tracing::error!("[db] failed to load, starting fresh");
                    let corrupt_path = format!("{}.corrupt-{}", file_path.display(), chrono::Utc::now().timestamp_millis());
                    let _ = std::fs::rename(&file_path, &corrupt_path);
                    needs_persist = true;
                    DbData::default()
                }
            }
        } else {
            needs_persist = true;
            DbData::default()
        };

        let db = Db { file_path, pid: std::process::id(), data };
        if needs_persist {
            db.persist();
        }
        db
    }

    /// Atomically persist current in-memory data (write to tmp, then rename).
    fn persist(&self) {
        let snapshot = match serde_json::to_string_pretty(&self.data) {
            Ok(s) => s,
            Err(err) => {
                tracing::error!("[db] persist failed (serialize): {}", err);
                return;
            }
        };
        let tmp = self.file_path.with_extension(format!("json.tmp-{}", self.pid));
        if let Err(err) = std::fs::write(&tmp, &snapshot) {
            tracing::error!("[db] persist failed: {}", err);
            return;
        }
        if let Err(err) = std::fs::rename(&tmp, &self.file_path) {
            tracing::error!("[db] persist failed: {}", err);
        }
    }

    // -----------------------------------------------------------------
    // Terminals
    // -----------------------------------------------------------------
    pub fn list_terminals(&self) -> Vec<Terminal> {
        self.data.terminals.clone()
    }

    pub fn get_terminal(&self, id: &str) -> Option<Terminal> {
        self.data.terminals.iter().find(|t| t.id == id).cloned()
    }

    pub fn create_terminal(&mut self, body: &Value) -> Terminal {
        let name = json_str_trim(body.get("name")).filter(|s| !s.is_empty()).unwrap_or_else(|| "Terminal".to_string());
        let model = body.get("model").and_then(Value::as_str).unwrap_or("A920 Pro").to_string();
        let conn_type = if body.get("connType").and_then(Value::as_str) == Some("usb") { "usb" } else { "tcp" }.to_string();
        let ip = json_str_trim(body.get("ip")).unwrap_or_default();
        let port = json_number(body.get("port")).map(|n| n as u16).filter(|&p| p != 0).unwrap_or(10009);
        let serial_path = json_str_trim(body.get("serialPath")).unwrap_or_default();
        let baud_rate = json_number(body.get("baudRate")).map(|n| n as u32).filter(|&b| b != 0).unwrap_or(115_200);

        let terminal = Terminal {
            id: Self::gen_id("term"),
            name,
            model,
            conn_type,
            ip,
            port,
            serial_path,
            baud_rate,
            created_at: chrono::Utc::now().to_rfc3339(),
        };
        self.data.terminals.push(terminal.clone());
        self.persist();
        terminal
    }

    pub fn update_terminal(&mut self, id: &str, patch: &Value) -> Option<Terminal> {
        let t = self.data.terminals.iter_mut().find(|t| t.id == id)?;
        if let Some(v) = patch.get("name") {
            if let Some(s) = v.as_str() {
                t.name = s.trim().to_string();
            }
        }
        if let Some(v) = patch.get("model") {
            if let Some(s) = v.as_str() {
                t.model = s.to_string();
            }
        }
        if let Some(v) = patch.get("connType") {
            t.conn_type = if v.as_str() == Some("usb") { "usb".to_string() } else { "tcp".to_string() };
        }
        if let Some(v) = patch.get("ip") {
            if let Some(s) = v.as_str() {
                t.ip = s.trim().to_string();
            }
        }
        if let Some(n) = json_number(patch.get("port")) {
            if n as u16 != 0 {
                t.port = n as u16;
            }
        }
        if let Some(v) = patch.get("serialPath") {
            if let Some(s) = v.as_str() {
                t.serial_path = s.trim().to_string();
            }
        }
        if let Some(n) = json_number(patch.get("baudRate")) {
            if n as u32 != 0 {
                t.baud_rate = n as u32;
            }
        }
        let result = t.clone();
        self.persist();
        Some(result)
    }

    pub fn delete_terminal(&mut self, id: &str) -> bool {
        let before = self.data.terminals.len();
        self.data.terminals.retain(|t| t.id != id);
        let deleted = self.data.terminals.len() != before;
        if deleted {
            self.persist();
        }
        deleted
    }

    // -----------------------------------------------------------------
    // ECR reference numbers -- unique, auto-incrementing, reset per day.
    // Format: <yyyymmdd><4-digit seq>, e.g. 202607100001
    // -----------------------------------------------------------------
    pub fn next_ecr_ref_num(&mut self) -> String {
        let now = chrono::Local::now();
        let date_key = now.format("%Y%m%d").to_string();
        let next = self.data.ecr_seq.get(&date_key).copied().unwrap_or(0) + 1;
        self.data.ecr_seq.insert(date_key.clone(), next);
        self.persist();
        format!("{}{:04}", date_key, next)
    }

    // -----------------------------------------------------------------
    // Transactions
    // -----------------------------------------------------------------
    pub fn create_transaction(&mut self, tx: Map<String, Value>) -> Map<String, Value> {
        let now = chrono::Utc::now().to_rfc3339();
        let mut record = Map::new();
        record.insert("id".into(), Value::String(Self::gen_id("txn")));
        record.insert("createdAt".into(), Value::String(now.clone()));
        record.insert("updatedAt".into(), Value::String(now));
        record.insert("status".into(), Value::String("PENDING".into()));
        for (k, v) in tx {
            record.insert(k, v);
        }
        self.data.transactions.insert(0, record.clone()); // newest first
        self.persist();
        record
    }

    pub fn update_transaction(&mut self, id: &str, patch: Map<String, Value>) -> Option<Map<String, Value>> {
        let tx = self
            .data
            .transactions
            .iter_mut()
            .find(|t| t.get("id").and_then(Value::as_str) == Some(id))?;
        for (k, v) in patch {
            tx.insert(k, v);
        }
        tx.insert("updatedAt".into(), Value::String(chrono::Utc::now().to_rfc3339()));
        let result = tx.clone();
        self.persist();
        Some(result)
    }

    pub fn get_transaction(&self, id: &str) -> Option<Map<String, Value>> {
        self.data.transactions.iter().find(|t| t.get("id").and_then(Value::as_str) == Some(id)).cloned()
    }

    pub fn query_transactions(&self, q: &TxQuery) -> TxQueryResult {
        let mut rows: Vec<&Map<String, Value>> = self.data.transactions.iter().collect();
        if let Some(s) = &q.status {
            rows.retain(|t| t.get("status").and_then(Value::as_str) == Some(s.as_str()));
        }
        if let Some(ty) = &q.tx_type {
            rows.retain(|t| t.get("type").and_then(Value::as_str) == Some(ty.as_str()));
        }
        if let Some(from) = &q.from {
            rows.retain(|t| t.get("createdAt").and_then(Value::as_str).map(|c| c >= from.as_str()).unwrap_or(false));
        }
        if let Some(to) = &q.to {
            rows.retain(|t| t.get("createdAt").and_then(Value::as_str).map(|c| c <= to.as_str()).unwrap_or(false));
        }
        if let Some(tid) = &q.terminal_id {
            rows.retain(|t| t.get("terminalId").and_then(Value::as_str) == Some(tid.as_str()));
        }

        let page = q.page.unwrap_or(1).max(1);
        let page_size = q.page_size.unwrap_or(25).clamp(1, 200);
        let total = rows.len();
        let start = ((page - 1) * page_size).max(0) as usize;
        let page_rows: Vec<Map<String, Value>> = rows.into_iter().skip(start).take(page_size as usize).cloned().collect();
        let page_count = if total == 0 { 1 } else { ((total as f64) / (page_size as f64)).ceil() as i64 };

        TxQueryResult { total, page, page_size, page_count, rows: page_rows }
    }

    /// Approved sale/auth/return transactions not yet batch-closed today.
    pub fn list_unsettled(&self) -> Vec<Map<String, Value>> {
        self.data
            .transactions
            .iter()
            .filter(|t| {
                let status_ok = t.get("status").and_then(Value::as_str) == Some("APPROVED");
                let not_settled = !t.get("settled").and_then(Value::as_bool).unwrap_or(false);
                let type_ok = matches!(t.get("type").and_then(Value::as_str), Some("SALE") | Some("AUTH") | Some("RETURN"));
                status_ok && not_settled && type_ok
            })
            .cloned()
            .collect()
    }

    /// Mark a set of transaction ids as settled (after a successful batch close).
    pub fn mark_settled(&mut self, ids: &[String], batch_info: Value) {
        let set: std::collections::HashSet<String> = ids.iter().cloned().collect();
        for tx in self.data.transactions.iter_mut() {
            if let Some(id) = tx.get("id").and_then(Value::as_str) {
                if set.contains(id) {
                    tx.insert("settled".into(), Value::Bool(true));
                    tx.insert("batchInfo".into(), batch_info.clone());
                    tx.insert("updatedAt".into(), Value::String(chrono::Utc::now().to_rfc3339()));
                }
            }
        }
        self.persist();
    }
}
