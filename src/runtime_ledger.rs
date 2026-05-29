//! Tamper-evident runtime receipt ledger.
//!
//! This is a local append-only JSONL hash chain. It is not a cryptocurrency or
//! consensus system. It gives Kcode a durable, verifiable receipt trail for
//! important runtime decisions such as governor mode changes, backend work,
//! session persistence, tool execution, and provider calls.

use anyhow::{Context, Result, anyhow};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

const LEDGER_FILE: &str = "runtime-ledger.jsonl";
const GENESIS_HASH: &str = "GENESIS";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeReceiptKind {
    Governor,
    BackendWork,
    SessionPersistence,
    ToolCall,
    ProviderCall,
}

impl Serialize for RuntimeReceiptKind {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> serde::Deserialize<'de> for RuntimeReceiptKind {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        match value.as_str() {
            "governor" => Ok(Self::Governor),
            "backend_work" => Ok(Self::BackendWork),
            "session_persistence" => Ok(Self::SessionPersistence),
            "tool_call" => Ok(Self::ToolCall),
            "provider_call" => Ok(Self::ProviderCall),
            other => Err(serde::de::Error::custom(format!(
                "unknown runtime receipt kind {other}"
            ))),
        }
    }
}

impl RuntimeReceiptKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Governor => "governor",
            Self::BackendWork => "backend_work",
            Self::SessionPersistence => "session_persistence",
            Self::ToolCall => "tool_call",
            Self::ProviderCall => "provider_call",
        }
    }
}

#[derive(Debug, Clone, Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct RuntimeReceipt {
    pub ts_ms: u128,
    pub kind: RuntimeReceiptKind,
    pub label: String,
    pub details: serde_json::Value,
    pub prev_hash: String,
    pub hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeLedgerVerification {
    pub entries: usize,
    pub head_hash: String,
}

pub fn default_path() -> PathBuf {
    crate::storage::runtime_dir().join(LEDGER_FILE)
}

pub fn append_receipt(
    kind: RuntimeReceiptKind,
    label: impl Into<String>,
    details: serde_json::Value,
) -> Result<RuntimeReceipt> {
    append_receipt_to(default_path(), kind, label, details)
}

pub fn append_receipt_best_effort(
    kind: RuntimeReceiptKind,
    label: impl Into<String>,
    details: serde_json::Value,
) {
    let _ = append_receipt(kind, label, details);
}

pub fn append_receipt_to(
    path: impl AsRef<Path>,
    kind: RuntimeReceiptKind,
    label: impl Into<String>,
    details: serde_json::Value,
) -> Result<RuntimeReceipt> {
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create runtime ledger dir {}", parent.display()))?;
    }
    let prev_hash = read_head_hash(path)?.unwrap_or_else(|| GENESIS_HASH.to_string());
    let mut receipt = RuntimeReceipt {
        ts_ms: now_ms(),
        kind,
        label: label.into(),
        details,
        prev_hash,
        hash: String::new(),
    };
    receipt.hash = compute_hash(&receipt)?;

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("failed to open runtime ledger {}", path.display()))?;
    serde_json::to_writer(&mut file, &receipt)?;
    file.write_all(b"\n")?;
    Ok(receipt)
}

pub fn verify_default() -> Result<RuntimeLedgerVerification> {
    verify(default_path())
}

pub fn format_verification(result: &RuntimeLedgerVerification) -> String {
    format!(
        "runtime ledger: entries={} head={}",
        result.entries, result.head_hash
    )
}

pub fn verify(path: impl AsRef<Path>) -> Result<RuntimeLedgerVerification> {
    let path = path.as_ref();
    if !path.exists() {
        return Ok(RuntimeLedgerVerification {
            entries: 0,
            head_hash: GENESIS_HASH.to_string(),
        });
    }

    let file = fs::File::open(path)
        .with_context(|| format!("failed to open runtime ledger {}", path.display()))?;
    let reader = BufReader::new(file);
    let mut prev = GENESIS_HASH.to_string();
    let mut entries = 0;
    for (idx, line) in reader.lines().enumerate() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let receipt: RuntimeReceipt = serde_json::from_str(&line)
            .with_context(|| format!("invalid runtime ledger json at line {}", idx + 1))?;
        if receipt.prev_hash != prev {
            return Err(anyhow!(
                "runtime ledger hash-chain break at line {}: expected prev {}, got {}",
                idx + 1,
                prev,
                receipt.prev_hash
            ));
        }
        let expected = compute_hash(&RuntimeReceipt {
            hash: String::new(),
            ..receipt.clone()
        })?;
        if receipt.hash != expected {
            return Err(anyhow!(
                "runtime ledger hash mismatch at line {}: expected {}, got {}",
                idx + 1,
                expected,
                receipt.hash
            ));
        }
        prev = receipt.hash;
        entries += 1;
    }

    Ok(RuntimeLedgerVerification {
        entries,
        head_hash: prev,
    })
}

fn read_head_hash(path: &Path) -> Result<Option<String>> {
    Ok(if path.exists() {
        let verification = verify(path)?;
        Some(verification.head_hash)
    } else {
        None
    })
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn compute_hash(receipt: &RuntimeReceipt) -> Result<String> {
    #[derive(Serialize)]
    struct HashMaterial<'a> {
        ts_ms: u128,
        kind: &'a str,
        label: &'a str,
        details: &'a serde_json::Value,
        prev_hash: &'a str,
    }
    let material = HashMaterial {
        ts_ms: receipt.ts_ms,
        kind: receipt.kind.as_str(),
        label: &receipt.label,
        details: &receipt.details,
        prev_hash: &receipt.prev_hash,
    };
    let bytes = serde_json::to_vec(&material)?;
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    Ok(format!("{:x}", hasher.finalize()))
}

static LEDGER_ENABLED: OnceLock<bool> = OnceLock::new();

pub fn enabled() -> bool {
    *LEDGER_ENABLED.get_or_init(|| {
        std::env::var("KCODE_RUNTIME_LEDGER")
            .map(|v| !matches!(v.as_str(), "0" | "false" | "off"))
            .unwrap_or(true)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_ledger_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "kcode-runtime-ledger-{name}-{}-{}.jsonl",
            std::process::id(),
            now_ms()
        ))
    }

    #[test]
    fn appends_and_verifies_hash_chain() {
        let path = temp_ledger_path("ok");
        append_receipt_to(
            &path,
            RuntimeReceiptKind::Governor,
            "policy",
            serde_json::json!({"mode":"normal"}),
        )
        .unwrap();
        append_receipt_to(
            &path,
            RuntimeReceiptKind::BackendWork,
            "queue",
            serde_json::json!({"len":1}),
        )
        .unwrap();
        let verification = verify(&path).unwrap();
        assert_eq!(verification.entries, 2);
        assert_ne!(verification.head_hash, GENESIS_HASH);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn formats_verification() {
        let text = format_verification(&RuntimeLedgerVerification {
            entries: 2,
            head_hash: "abc".to_string(),
        });
        assert!(text.contains("entries=2"));
        assert!(text.contains("head=abc"));
    }

    #[test]
    fn detects_tampering() {
        let path = temp_ledger_path("tamper");
        append_receipt_to(
            &path,
            RuntimeReceiptKind::ToolCall,
            "bash",
            serde_json::json!({"ok":true}),
        )
        .unwrap();
        let mut text = fs::read_to_string(&path).unwrap();
        text = text.replace("true", "false");
        fs::write(&path, text).unwrap();
        assert!(verify(&path).is_err());
        let _ = fs::remove_file(path);
    }
}
