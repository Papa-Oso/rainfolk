//! Finite, sequential local inventory jobs. No scheduler or artifact activation.
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const MAX_BYTES: usize = 1024 * 1024;
#[path = "../page_segments.rs"]
mod page_segments;
#[path = "../job_partitions.rs"]
mod partitions;
#[path = "../sdr_bulk.rs"]
mod sdr_bulk;
#[path = "../sdr_jobs.rs"]
mod sdr_jobs;
#[path = "../usgwd.rs"]
mod usgwd;
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn encode(value: &Value) -> Result<Vec<u8>, String> {
    serde_json::to_vec(value).map_err(|e| e.to_string())
}
fn read_capped(path: &Path, cap: usize) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|e| e.to_string())?
        .take(cap as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > cap {
        return Err("file byte budget exceeded".into());
    }
    Ok(bytes)
}
fn number(value: &Value, key: &str, min: u64, max: u64) -> Result<u64, String> {
    value[key]
        .as_u64()
        .filter(|n| (min..=max).contains(n))
        .ok_or_else(|| format!("invalid budget: {key}"))
}
fn validate_config(config: &Value) -> Result<(), String> {
    if config["adapter"] == "texas-sdr" {
        return sdr_jobs::validate_config(config);
    }
    if config["schemaVersion"] == 2 {
        return partitions::validate_config(config);
    }
    if config["schemaVersion"] != 1
        || config["adapter"] != "usgs-monitoring-locations"
        || !["fixture", "live"]
            .iter()
            .any(|mode| config["mode"] == *mode)
    {
        return Err("unsupported job config/adapter".into());
    }
    let states = config["states"]
        .as_array()
        .ok_or("states must be an array")?;
    if states.is_empty() || states.len() > 56 {
        return Err("finite queue requires 1..56 states".into());
    }
    let mut seen = std::collections::BTreeSet::new();
    for item in states {
        let state = item["state"].as_str().ok_or("missing state")?;
        if state.len() != 2 || !state.bytes().all(|b| b.is_ascii_digit()) || !seen.insert(state) {
            return Err("invalid/duplicate string state code".into());
        }
    }
    let budgets = &config["budgets"];
    number(budgets, "pages", 1, 2)?;
    number(budgets, "rowsPerPage", 1, 25)?;
    number(budgets, "bytesPerState", 1024, MAX_BYTES as u64)?;
    number(budgets, "stateSeconds", 1, 90)?;
    number(budgets, "runSeconds", 1, 300)?;
    number(budgets, "rateMillis", 1000, 10000)?;
    Ok(())
}
fn checkpoint(path: &Path, config: &Value) -> Result<Value, String> {
    let fingerprint = hash(&encode(config)?);
    if path.exists() {
        let envelope: Value =
            serde_json::from_slice(&read_capped(path, MAX_BYTES)?).map_err(|e| e.to_string())?;
        let payload = &envelope["payload"];
        if envelope["schemaVersion"] != 1
            || envelope["sha256"] != hash(&encode(payload)?)
            || payload["configSha256"] != fingerprint
        {
            return Err("checkpoint integrity/config mismatch; refusing resume".into());
        }
        let states = payload["states"]
            .as_object()
            .ok_or("invalid checkpoint states")?;
        if states.len() != config["states"].as_array().unwrap().len() {
            return Err("checkpoint state count mismatch".into());
        }
        for item in config["states"].as_array().unwrap() {
            let code = item["state"].as_str().unwrap();
            let record = &payload["states"][code];
            if !["pending", "running", "completed", "partial", "failed"]
                .iter()
                .any(|status| record["status"] == *status)
                || record["attempts"].as_u64().is_none()
            {
                return Err("invalid checkpoint status".into());
            }
            if let Some(name) = record["candidate"].as_str() {
                let digest = record["candidateSha256"]
                    .as_str()
                    .ok_or("candidate hash missing")?;
                if digest.len() != 64
                    || !digest.bytes().all(|b| b.is_ascii_hexdigit())
                    || name != format!("{code}-candidate-{digest}.json")
                {
                    return Err("invalid candidate reference".into());
                }
                let bytes = read_capped(&path.parent().unwrap().join(name), 16 * MAX_BYTES)?;
                if hash(&bytes) != digest {
                    return Err("last-good candidate integrity failure".into());
                }
                let bundle: Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
                gev_us_groundwater::validate_bundle(&bundle)?;
                let capture_digest = bundle["manifest"]["input"]["sha256"]
                    .as_str()
                    .ok_or("capture hash missing")?;
                let capture_name = format!("{code}-capture-{capture_digest}.json");
                if record["capture"] != capture_name {
                    return Err("invalid capture reference".into());
                }
                if hash(&read_capped(
                    &path.parent().unwrap().join(&capture_name),
                    MAX_BYTES,
                )?) != capture_digest
                {
                    return Err("capture provenance integrity failure".into());
                }
            }
        }
        return Ok(payload.clone());
    }
    let states: serde_json::Map<String, Value> = config["states"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| {
            (
                item["state"].as_str().unwrap().to_owned(),
                json!({"status":"pending", "attempts":0}),
            )
        })
        .collect();
    Ok(json!({"configSha256":fingerprint,"states":states,"generation":0}))
}
fn save(path: &Path, payload: &mut Value) -> Result<(), String> {
    payload["generation"] = json!(payload["generation"].as_u64().unwrap_or(0) + 1);
    let bytes =
        encode(&json!({"schemaVersion":1,"sha256":hash(&encode(payload)?),"payload":payload}))?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    let temporary = path.with_file_name(format!("checkpoint.{}.{}.tmp", std::process::id(), nonce));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|e| e.to_string())?;
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(|e| e.to_string())?;
    drop(file);
    fs::rename(temporary, path).map_err(|e| e.to_string())
}
fn immutable(dir: &Path, kind: &str, bytes: &[u8]) -> Result<String, String> {
    let name = format!("{kind}-{}.json", hash(bytes));
    let path = dir.join(&name);
    if path.exists() {
        if read_capped(&path, 16 * MAX_BYTES)? != bytes {
            return Err("immutable artifact integrity failure".into());
        }
    } else {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_nanos();
        let temporary = dir.join(format!("artifact.{}.{}.tmp", std::process::id(), nonce));
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
            .map_err(|e| e.to_string())?;
        file.write_all(bytes)
            .and_then(|_| file.sync_all())
            .map_err(|e| e.to_string())?;
        drop(file);
        fs::rename(temporary, &path).map_err(|e| e.to_string())?;
    }
    Ok(name)
}

/// Adapters return captured source bytes. Admission/export stays in the Rust core.
trait InventoryAdapter {
    fn metrics(&self) -> Value {
        Value::Null
    }
    fn capture(
        &mut self,
        item: &Value,
        budgets: &Value,
        cache: &Path,
        seconds: u64,
    ) -> Result<Vec<u8>, String>;
}
struct SourceAdapter {
    repo: PathBuf,
    live: bool,
    metrics: Value,
}
impl InventoryAdapter for SourceAdapter {
    fn metrics(&self) -> Value {
        self.metrics.clone()
    }
    fn capture(
        &mut self,
        item: &Value,
        budgets: &Value,
        cache: &Path,
        seconds: u64,
    ) -> Result<Vec<u8>, String> {
        let cap = budgets["bytesPerState"].as_u64().unwrap() as usize;
        self.metrics = Value::Null;
        if !self.live {
            let fixture = item["fixturePartitions"]
                .as_array()
                .and_then(|files| files.get(item["_partitionIndex"].as_u64().unwrap_or(0) as usize))
                .and_then(Value::as_str)
                .or_else(|| item["fixture"].as_str())
                .ok_or("fixture partition path required")?;
            let relative = Path::new(fixture);
            if relative
                .components()
                .any(|c| !matches!(c, std::path::Component::Normal(_)))
            {
                return Err("fixture must be repository-relative".into());
            }
            let path = self
                .repo
                .join(relative)
                .canonicalize()
                .map_err(|e| e.to_string())?;
            if !path.starts_with(self.repo.canonicalize().map_err(|e| e.to_string())?) {
                return Err("fixture outside repository".into());
            }
            let bytes = read_capped(&path, cap)?;
            if item["_sdrAfterObjectId"].is_u64() {
                sdr_jobs::stage_fixture(cache, &bytes)?;
            }
            return Ok(bytes);
        }
        let sdr = item["_sdrAfterObjectId"].as_u64();
        let mut command = Command::new("python");
        command.arg("-B");
        if let Some(after) = sdr {
            command
                .arg(self.repo.join("scripts/capture-texas-sdr.py"))
                .arg("--public-sample")
                .args([
                    "--after-object-id",
                    &after.to_string(),
                    "--record-limit",
                    &budgets["rowsPerPage"].to_string(),
                    "--max-seconds",
                    &seconds.saturating_sub(1).min(60).max(1).to_string(),
                    "--date-omission-evidence-sha256",
                    item["_sdrEvidence"].as_str().unwrap(),
                ])
                .arg("--output")
                .arg(cache);
        } else {
            command
                .arg(self.repo.join("scripts/capture-usgs-inventory.py"))
                .args([
                    "--state",
                    item["state"].as_str().unwrap(),
                    "--pages",
                    &budgets["pages"].to_string(),
                    "--limit",
                    &budgets["rowsPerPage"].to_string(),
                    "--page-bytes",
                    &budgets["_pageBytes"]
                        .as_u64()
                        .unwrap_or(512 * 1024)
                        .to_string(),
                    "--max-bytes",
                    &cap.to_string(),
                    "--rate-millis",
                    &budgets["rateMillis"].to_string(),
                ])
                .args(
                    budgets["projection"]
                        .as_str()
                        .map(|value| ["--properties", value])
                        .into_iter()
                        .flatten(),
                )
                .args(
                    item["_startUrl"]
                        .as_str()
                        .map(|url| ["--start-url", url])
                        .into_iter()
                        .flatten(),
                )
                .arg("--cache-dir")
                .arg(cache);
        }
        let mut child = command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| e.to_string())?;
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        let out = thread::spawn(move || {
            let mut bytes = Vec::new();
            stdout
                .take(cap as u64 + 1)
                .read_to_end(&mut bytes)
                .map(|_| bytes)
        });
        let err = thread::spawn(move || {
            let mut bytes = Vec::new();
            stderr.take(4096).read_to_end(&mut bytes).map(|_| bytes)
        });
        let started = Instant::now();
        let status = loop {
            if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
                break status;
            }
            if started.elapsed() >= Duration::from_secs(seconds) {
                let _ = child.kill();
                let _ = child.wait();
                let _ = out.join();
                let _ = err.join();
                return Err("state/run wall-time budget reached; bounded child stopped".into());
            }
            thread::sleep(Duration::from_millis(50));
        };
        let bytes = out
            .join()
            .map_err(|_| "capture reader failed")?
            .map_err(|e| e.to_string())?;
        let errors = err
            .join()
            .map_err(|_| "capture stderr failed")?
            .map_err(|e| e.to_string())?;
        self.metrics = String::from_utf8_lossy(&errors)
            .lines()
            .find_map(|line| {
                line.strip_prefix("USGS_METRICS ")
                    .and_then(|value| serde_json::from_str(value).ok())
            })
            .unwrap_or(Value::Null);
        if sdr.is_some() {
            self.metrics = serde_json::from_slice(&bytes).unwrap_or_else(|_| {
                read_capped(&cache.join("failure.json"), MAX_BYTES)
                    .ok()
                    .and_then(|b| serde_json::from_slice(&b).ok())
                    .unwrap_or(Value::Null)
            });
            if !status.success() {
                return Err("SDR transport failed; stopped without retry".into());
            }
            return sdr_jobs::cached_capture(cache, false);
        }
        if !status.success() {
            return Err(String::from_utf8_lossy(&errors).into_owned());
        }
        if bytes.len() > cap {
            return Err("transport stdout byte budget exceeded".into());
        }
        Ok(bytes)
    }
}
fn run(config: &Value, dir: &Path, adapter: &mut dyn InventoryAdapter) -> Result<Value, String> {
    validate_config(config)?;
    if config["mode"] == "live" {
        return Err("historical v1 live runner has no persisted expiry; use v2 for new bounded jobs; status remains available".into());
    }
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join("runner.lock"))
        .map_err(|e| e.to_string())?;
    lock.try_lock()
        .map_err(|_| "another runner owns this checkpoint")?;
    let path = dir.join("checkpoint.json");
    let mut state = checkpoint(&path, config)?;
    if config["mode"] == "live" && state["networkBlocked"] == true {
        return Err("previous network access denial; automatic resume refused".into());
    }
    let started = Instant::now();
    let budgets = &config["budgets"];
    let run_seconds = budgets["runSeconds"].as_u64().unwrap();
    for item in config["states"].as_array().unwrap() {
        let code = item["state"].as_str().unwrap();
        let previous = state["states"][code].clone();
        if ["completed", "partial"]
            .iter()
            .any(|s| previous["status"] == *s)
            || previous["blocked"] == true
        {
            continue;
        }
        if config["mode"] == "live" {
            thread::sleep(Duration::from_millis(
                budgets["rateMillis"].as_u64().unwrap(),
            ));
        }
        let remaining = run_seconds.saturating_sub(started.elapsed().as_secs());
        if remaining == 0 {
            break;
        }
        let seconds = remaining.min(budgets["stateSeconds"].as_u64().unwrap());
        state["states"][code]["status"] = json!("running");
        state["states"][code]["attempts"] = json!(previous["attempts"].as_u64().unwrap_or(0) + 1);
        save(&path, &mut state)?;
        let attempt: Result<(usize, Value, String, String), String> = (|| {
            let bytes =
                adapter.capture(item, budgets, &dir.join(format!("cache-{code}")), seconds)?;
            let capture: Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
            let pages = capture["acquisitions"]
                .as_array()
                .ok_or("capture pages missing")?;
            if bytes.len() > budgets["bytesPerState"].as_u64().unwrap() as usize
                || pages.len() > budgets["pages"].as_u64().unwrap() as usize
                || pages.iter().any(|p| {
                    p["response"]["features"]
                        .as_array()
                        .is_none_or(|r| r.len() > budgets["rowsPerPage"].as_u64().unwrap() as usize)
                })
            {
                return Err("configured capture budget exceeded".into());
            }
            let bundle = gev_us_groundwater::inventory_bundle(&bytes, code)?;
            gev_us_groundwater::validate_bundle(&bundle)?;
            let count = bundle["inventory"]["records"].as_array().unwrap().len();
            let next = pages
                .last()
                .and_then(|p| p["response"]["links"].as_array())
                .and_then(|l| l.iter().find(|v| v["rel"] == "next"))
                .map(|v| v["href"].clone())
                .unwrap_or(Value::Null);
            let capture_file = immutable(dir, &format!("{code}-capture"), &bytes)?;
            let candidate = immutable(dir, &format!("{code}-candidate"), &encode(&bundle)?)?;
            Ok((count, next, capture_file, candidate))
        })();
        match attempt {
            Ok((count, next, capture_file, candidate)) => {
                state["states"][code] = json!({"status":if next.is_null(){"completed"}else{"partial"},"attempts":state["states"][code]["attempts"],"accepted":count,"observations":0,"nextPage":next,"capture":capture_file,"candidate":candidate,"candidateSha256":hash(&read_capped(&dir.join(&candidate),16*MAX_BYTES)?),"activation":"candidate-only","error":null});
            }
            Err(error) => {
                let blocked = ["403", "401", "Permission denied", "Access is denied"]
                    .iter()
                    .any(|s| error.contains(s));
                state["states"][code]["status"] = json!("failed");
                state["states"][code]["error"] = json!(error);
                state["states"][code]["blocked"] = json!(blocked);
                if blocked {
                    state["networkBlocked"] = json!(true);
                }
                save(&path, &mut state)?;
                if blocked {
                    break;
                }
                continue;
            }
        }
        save(&path, &mut state)?;
    }
    if !path.exists() {
        save(&path, &mut state)?;
    }
    Ok(state)
}
fn main() {
    if let Err(error) = cli() {
        eprintln!("well jobs refused: {error}");
        std::process::exit(1);
    }
}
fn cli() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|s| s == "import-usgwd-segmented") {
        if !(4..=6).contains(&args.len()) {
            return Err("usage: well-jobs import-usgwd-segmented <csv> <source.json> <directory> [rows-per-segment] [new-partitions]; one new segment per invocation".into());
        }
        let rows = args
            .get(4)
            .map(|s| s.parse().map_err(|_| "invalid segment rows"))
            .transpose()?
            .unwrap_or(125000);
        let allowance = args
            .get(5)
            .map(|s| s.parse().map_err(|_| "invalid partition allowance"))
            .transpose()?;
        let result = usgwd::segments::run(
            Path::new(&args[1]),
            Path::new(&args[2]),
            Path::new(&args[3]),
            rows,
            allowance,
        )?;
        println!("{result}");
        return Ok(());
    }
    if args.first().is_some_and(|s| s == "import-usgwd") {
        if !(4..=5).contains(&args.len()) {
            return Err(
                "usage: well-jobs import-usgwd <csv> <source.json> <directory> [full|new-partitions]; default full"
                    .into(),
            );
        }
        let (csv, receipt, output) = (
            Path::new(&args[1]),
            Path::new(&args[2]),
            Path::new(&args[3]),
        );
        let result = match args.get(4).map(String::as_str) {
            None | Some("full") => usgwd::run_full(csv, receipt, output)?,
            Some(n) => usgwd::run(
                csv,
                receipt,
                output,
                n.parse().map_err(|_| "invalid partition allowance")?,
            )?,
        };
        println!("{result}");
        return Ok(());
    }
    if args.first().is_some_and(|s| s == "import-sdr-bulk") {
        if args.len() != 2 {
            return Err(
                "usage: well-jobs import-sdr-bulk <fresh-directory>; bounded JSONL on stdin".into(),
            );
        }
        let result = sdr_bulk::ingest(Path::new(&args[1]), &mut std::io::stdin().lock())?;
        println!(
            "{}",
            serde_json::to_string(&result).map_err(|_| "bulk summary encoding")?
        );
        return Ok(());
    }
    if args.first().is_some_and(|s| s == "segment-page") {
        if args.len() != 3 {
            return Err(
                "usage: well-jobs segment-page <page-config.json> <segment-directory>".into(),
            );
        }
        let config: Value = serde_json::from_slice(&read_capped(Path::new(&args[1]), 65536)?)
            .map_err(|e| e.to_string())?;
        let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let state = page_segments::ingest(&repo, &config, Path::new(&args[2]))?;
        println!(
            "{}",
            serde_json::to_string_pretty(&state).map_err(|e| e.to_string())?
        );
        return Ok(());
    }
    if args.first().is_some_and(|s| s == "amend") {
        if args.len() != 5 {
            return Err("usage: well-jobs amend <old-queue> <job-directory> <new-queue> <absolute-expiry-unix-seconds>".into());
        }
        let old: Value = serde_json::from_slice(&read_capped(Path::new(&args[1]), 65536)?)
            .map_err(|e| e.to_string())?;
        let new: Value = serde_json::from_slice(&read_capped(Path::new(&args[3]), 65536)?)
            .map_err(|e| e.to_string())?;
        let state = partitions::amend(
            &old,
            Path::new(&args[2]),
            &new,
            args[4].parse().map_err(|_| "invalid expiry")?,
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|e| e.to_string())?
                .as_secs(),
        )?;
        println!(
            "{}",
            serde_json::to_string_pretty(&state).map_err(|e| e.to_string())?
        );
        return Ok(());
    }
    if args.first().is_some_and(|s| s == "migrate") {
        if args.len() != 7 {
            return Err("usage: well-jobs migrate <old-queue> <source-job> <new-queue> <fresh-target-job> <expiry-unix-seconds> <recovery-directory>".into());
        }
        let old: Value = serde_json::from_slice(&read_capped(Path::new(&args[1]), 65536)?)
            .map_err(|e| e.to_string())?;
        let new: Value = serde_json::from_slice(&read_capped(Path::new(&args[3]), 65536)?)
            .map_err(|e| e.to_string())?;
        let state = partitions::migrate(
            &old,
            Path::new(&args[2]),
            &new,
            Path::new(&args[4]),
            args[5].parse().map_err(|_| "invalid expiry")?,
            Some(Path::new(&args[6])),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|e| e.to_string())?
                .as_secs(),
        )?;
        println!(
            "{}",
            serde_json::to_string_pretty(&state).map_err(|e| e.to_string())?
        );
        return Ok(());
    }
    if args.len() != 3 || !["run", "resume", "status"].contains(&args[0].as_str()) {
        return Err("usage: well-jobs <run|resume|status> <queue.json> <work-directory>".into());
    }
    let config: Value = serde_json::from_slice(&read_capped(Path::new(&args[1]), 65536)?)
        .map_err(|e| e.to_string())?;
    validate_config(&config)?;
    let dir = Path::new(&args[2]);
    let result = if config["adapter"] == "texas-sdr" {
        sdr_jobs::execute(
            &config,
            dir,
            args[0] == "status",
            &mut SourceAdapter {
                repo: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."),
                live: config["mode"] == "live",
                metrics: Value::Null,
            },
        )?
    } else if config["schemaVersion"] == 2 {
        let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        partitions::execute(
            &config,
            dir,
            args[0] == "status",
            &mut SourceAdapter {
                repo,
                live: config["mode"] == "live",
                metrics: Value::Null,
            },
        )?
    } else if args[0] == "status" {
        checkpoint(&dir.join("checkpoint.json"), &config)?
    } else {
        let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        run(
            &config,
            dir,
            &mut SourceAdapter {
                repo,
                live: config["mode"] == "live",
                metrics: Value::Null,
            },
        )?
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&result).map_err(|e| e.to_string())?
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn scratch() -> PathBuf {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../.temp-gev")
            .join(format!(
                "jobs-test-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
        fs::create_dir_all(&path).unwrap();
        path
    }
    fn config(states: &[&str]) -> Value {
        json!({"schemaVersion":1,"adapter":"usgs-monitoring-locations","mode":"fixture","budgets":{"pages":2,"rowsPerPage":25,"bytesPerState":1048576,"stateSeconds":10,"runSeconds":30,"rateMillis":1000},"states":states.iter().map(|s|json!({"state":s})).collect::<Vec<_>>()})
    }
    struct Fixture {
        calls: usize,
        panic_once: bool,
        fail: Option<String>,
    }
    impl InventoryAdapter for Fixture {
        fn capture(
            &mut self,
            item: &Value,
            _: &Value,
            _: &Path,
            _: u64,
        ) -> Result<Vec<u8>, String> {
            self.calls += 1;
            if self.panic_once {
                self.panic_once = false;
                panic!("simulated process interruption after running checkpoint");
            }
            if let Some(error) = &self.fail {
                return Err(error.clone());
            }
            match item["state"].as_str().unwrap() {
                "36" => Ok(include_bytes!(
                    "../../../../fixtures/us-groundwater/usgs-inventory-pilot/capture.json"
                )
                .to_vec()),
                "22" => Ok(include_bytes!(
                    "../../../../fixtures/us-groundwater/jobs-v1/la-capture.json"
                )
                .to_vec()),
                _ => Err("synthetic state source failed".into()),
            }
        }
    }
    fn adapter() -> Fixture {
        Fixture {
            calls: 0,
            panic_once: false,
            fail: None,
        }
    }
    #[test]
    fn one_failed_state_does_not_abort_others_or_duplicate_resume_counts() {
        let dir = scratch();
        let config = config(&["22", "55", "36"]);
        let mut adapter = adapter();
        let first = run(&config, &dir, &mut adapter).unwrap();
        assert_eq!(first["states"]["22"]["accepted"], 50);
        assert_eq!(first["states"]["22"]["status"], "partial");
        assert_eq!(first["states"]["55"]["status"], "failed");
        assert_eq!(first["states"]["36"]["accepted"], 1);
        assert!(
            first["states"]["22"]["nextPage"]
                .as_str()
                .unwrap()
                .contains("cursor=")
        );
        let second = run(&config, &dir, &mut adapter).unwrap();
        assert_eq!(adapter.calls, 4);
        assert_eq!(first["states"]["22"], second["states"]["22"]);
        assert_eq!(first["states"]["36"], second["states"]["36"]);
    }
    #[test]
    fn completed_queue_is_byte_identical_noop() {
        let dir = scratch();
        let config = config(&["36"]);
        let mut adapter = adapter();
        run(&config, &dir, &mut adapter).unwrap();
        let before = fs::read(dir.join("checkpoint.json")).unwrap();
        fs::write(
            dir.join("checkpoint.interrupted.tmp"),
            b"incomplete temporary write",
        )
        .unwrap();
        run(&config, &dir, &mut adapter).unwrap();
        assert_eq!(adapter.calls, 1);
        assert_eq!(before, fs::read(dir.join("checkpoint.json")).unwrap());
    }
    #[test]
    fn interrupted_running_state_resumes_without_double_counting() {
        let dir = scratch();
        let config = config(&["36"]);
        let mut adapter = adapter();
        adapter.panic_once = true;
        let interrupted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run(&config, &dir, &mut adapter)
        }));
        assert!(interrupted.is_err());
        assert_eq!(
            checkpoint(&dir.join("checkpoint.json"), &config).unwrap()["states"]["36"]["status"],
            "running"
        );
        let result = run(&config, &dir, &mut adapter).unwrap();
        assert_eq!(result["states"]["36"]["accepted"], 1);
        assert_eq!(result["states"]["36"]["attempts"], 2);
    }
    #[test]
    fn corrupt_checkpoint_or_changed_config_is_refused_before_adapter() {
        let dir = scratch();
        let config = config(&["36"]);
        let mut adapter = adapter();
        run(&config, &dir, &mut adapter).unwrap();
        let mut changed = config.clone();
        changed["budgets"]["pages"] = json!(1);
        assert!(run(&changed, &dir, &mut adapter).is_err());
        let path = dir.join("checkpoint.json");
        let mut envelope: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        envelope["payload"]["states"]["36"]["accepted"] = json!(999);
        fs::write(&path, encode(&envelope).unwrap()).unwrap();
        assert!(run(&config, &dir, &mut adapter).is_err());
        assert_eq!(adapter.calls, 1);
    }
    #[test]
    fn failure_retains_last_valid_candidate_and_tampering_fails_closed() {
        let dir = scratch();
        let config = config(&["36"]);
        let mut adapter = adapter();
        let mut state = run(&config, &dir, &mut adapter).unwrap();
        let file = state["states"]["36"]["candidate"]
            .as_str()
            .unwrap()
            .to_owned();
        let bytes = fs::read(dir.join(&file)).unwrap();
        state["states"]["36"]["status"] = json!("pending");
        save(&dir.join("checkpoint.json"), &mut state).unwrap();
        adapter.fail = Some("synthetic validation failure".into());
        let failed = run(&config, &dir, &mut adapter).unwrap();
        assert_eq!(failed["states"]["36"]["status"], "failed");
        assert_eq!(failed["states"]["36"]["candidate"], file);
        assert_eq!(bytes, fs::read(dir.join(&file)).unwrap());
        fs::write(dir.join(&file), b"tampered test artifact").unwrap();
        assert!(run(&config, &dir, &mut adapter).is_err());
    }
    #[test]
    fn strict_queue_and_page_budgets_fail_without_a_candidate() {
        let dir = scratch();
        let mut config = config(&["22"]);
        let mut adapter = adapter();
        config["budgets"]["pages"] = json!(1);
        let result = run(&config, &dir, &mut adapter).unwrap();
        assert_eq!(result["states"]["22"]["status"], "failed");
        assert!(result["states"]["22"]["candidate"].is_null());
        config["budgets"]["pages"] = json!(0);
        assert!(validate_config(&config).is_err());
        config["budgets"]["pages"] = json!(2);
        config["states"] = json!([{"state":"36"},{"state":"36"}]);
        assert!(validate_config(&config).is_err());
    }
    #[test]
    fn access_denial_stops_remaining_actions_and_is_marked_blocked() {
        let dir = scratch();
        let config = config(&["36", "22"]);
        let mut adapter = adapter();
        adapter.fail = Some("HTTP Error 403: Forbidden".into());
        let result = run(&config, &dir, &mut adapter).unwrap();
        assert_eq!(adapter.calls, 1);
        assert_eq!(result["networkBlocked"], true);
        assert_eq!(result["states"]["22"]["status"], "pending");
    }
    #[test]
    fn concurrent_owner_refused_without_changing_checkpoint() {
        let dir = scratch();
        let config = config(&["36"]);
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(dir.join("runner.lock"))
            .unwrap();
        lock.try_lock().unwrap();
        assert!(run(&config, &dir, &mut adapter()).is_err());
        assert!(!dir.join("checkpoint.json").exists());
    }

    #[test]
    fn capture_provenance_tampering_is_refused_before_resume() {
        let dir = scratch();
        let config = config(&["36"]);
        let mut adapter = adapter();
        let state = run(&config, &dir, &mut adapter).unwrap();
        let name = state["states"]["36"]["capture"].as_str().unwrap();
        fs::write(dir.join(name), b"corrupt test capture").unwrap();
        assert!(run(&config, &dir, &mut adapter).is_err());
        assert_eq!(adapter.calls, 1);
    }

    #[test]
    fn elapsed_run_budget_leaves_unstarted_states_pending() {
        struct SlowFixture {
            calls: usize,
        }
        impl InventoryAdapter for SlowFixture {
            fn capture(
                &mut self,
                item: &Value,
                budgets: &Value,
                cache: &Path,
                seconds: u64,
            ) -> Result<Vec<u8>, String> {
                self.calls += 1;
                assert_eq!(seconds, 1);
                thread::sleep(Duration::from_millis(1100));
                adapter().capture(item, budgets, cache, seconds)
            }
        }
        let dir = scratch();
        let mut config = config(&["36", "22"]);
        config["budgets"]["runSeconds"] = json!(1);
        let mut adapter = SlowFixture { calls: 0 };
        let result = run(&config, &dir, &mut adapter).unwrap();
        assert_eq!(adapter.calls, 1);
        assert_eq!(result["states"]["22"]["status"], "pending");
    }
}
