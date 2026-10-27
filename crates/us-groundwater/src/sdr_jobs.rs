//! Small SDR branch of well-jobs; shared adapter, locks and atomic artifact writers.
use super::*;
use gev_us_groundwater::texas_sdr::{normalize, public_job_query_url};
use std::collections::BTreeMap;
const CHARGE: u64 = MAX_BYTES as u64;

pub(super) fn validate_config(c: &Value) -> Result<(), String> {
    if c["schemaVersion"] != 1
        || c["adapter"] != "texas-sdr"
        || !["fixture", "live"].iter().any(|m| c["mode"] == *m)
        || c["dateOmissionEvidenceSha256"]
            .as_str()
            .is_none_or(|s| s.len() != 64 || !s.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        return Err("invalid SDR job config/evidence".into());
    }
    let b = &c["budgets"];
    number(b, "rowsPerPage", 1, 200)?;
    number(b, "runPages", 1, 10)?;
    number(b, "jobPages", 1, 10)?;
    number(b, "jobRequests", 2, 20)?;
    number(b, "jobBytes", CHARGE, 10 * CHARGE)?;
    number(b, "jobRows", 1, 2000)?;
    number(b, "jobSeconds", 2, 3600)?;
    number(b, "runSeconds", 2, 60)?;
    number(b, "rateMillis", 1000, 10000)?;
    if b["runPages"].as_u64() > b["jobPages"].as_u64() {
        return Err("run pages exceed lifetime pages".into());
    }
    if c["mode"] == "fixture"
        && c["fixturePages"].as_array().is_none_or(|p| {
            p.len() != b["jobPages"].as_u64().unwrap() as usize || p.iter().any(|v| !v.is_string())
        })
    {
        return Err("SDR fixture page paths required".into());
    }
    Ok(())
}

fn persist(path: &Path, s: &mut Value) -> Result<(), String> {
    if encode(s)?.len() > MAX_BYTES - 1024 {
        return Err("SDR checkpoint byte cap".into());
    }
    save(path, s)
}

// Fixture mode uses the same durable-cache boundary as the Python transport.
pub(super) fn stage_fixture(cache: &Path, raw: &[u8]) -> Result<(), String> {
    fs::create_dir_all(cache).map_err(|e| e.to_string())?;
    fs::write(cache.join("capture.json"), raw).map_err(|e| e.to_string())?;
    for name in ["schema.json", "public-response.json"] {
        fs::write(cache.join(name), b"{}").map_err(|e| e.to_string())?;
    }
    fs::write(cache.join("evidence.json"),encode(&json!({"fixture":true,"captureSha256":hash(raw),"networkRequests":0,"responseBytes":0,"schemaSha256":hash(b"{}"),"responseSha256":hash(b"{}")}))?).map_err(|e|e.to_string())
}

pub(super) fn cached_capture(cache: &Path, fixture: bool) -> Result<Vec<u8>, String> {
    let raw = read_capped(&cache.join("capture.json"), MAX_BYTES)?;
    let e: Value = serde_json::from_slice(&read_capped(&cache.join("evidence.json"), MAX_BYTES)?)
        .map_err(|_| "invalid SDR cache receipt")?;
    if e["captureSha256"] != hash(&raw)
        || e["networkRequests"] != if fixture { json!(0) } else { json!(2) }
        || (e["fixture"] == true && !fixture)
        || e["responseBytes"].as_u64().is_none_or(|n| n > CHARGE)
    {
        return Err("SDR cache integrity/accounting failure".into());
    }
    let input: Value = serde_json::from_slice(&raw).map_err(|_| "invalid SDR cache capture")?;
    let mut total = 0;
    for (name, key) in [
        ("schema.json", "schemaSha256"),
        ("public-response.json", "responseSha256"),
    ] {
        let digest = e[key].as_str().ok_or("missing SDR source evidence hash")?;
        let input_key = if key == "schemaSha256" {
            "sourceSchemaSha256"
        } else {
            "sourceResponseSha256"
        };
        let bytes = read_capped(&cache.join(name), MAX_BYTES / 2)?;
        total += bytes.len() as u64;
        if hash(&bytes) != digest || input[input_key] != digest {
            return Err("SDR source evidence integrity failure".into());
        }
    }
    if !fixture && e["responseBytes"] != total {
        return Err("SDR source byte accounting mismatch".into());
    }
    Ok(raw)
}

#[derive(Default)]
pub(super) struct Reports {
    pub(super) ids: BTreeMap<String, (Option<String>, u64, bool)>,
    anonymous: u64,
}
impl Reports {
    fn add(&mut self, input: &Value, candidate: &Value) {
        let records: BTreeMap<_, _> = candidate["payload"]["records"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                (
                    r["id"].as_str().unwrap().to_owned(),
                    (
                        r["contentSha256"].as_str().unwrap().to_owned(),
                        r["mapEligible"] == true,
                    ),
                )
            })
            .collect();
        let rejected: BTreeMap<_, _> = candidate["payload"]["quarantine"]
            .as_array()
            .unwrap()
            .iter()
            .map(|q| {
                (
                    q["rowIndex"].as_u64().unwrap() as usize,
                    q["sourceId"].as_str().map(str::to_owned),
                )
            })
            .collect();
        for (n, row) in input["response"]["features"]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
        {
            let (id, content, mapped) = if let Some(id) = rejected.get(&n) {
                (id.clone(), None, false)
            } else {
                let id = format!(
                    "TWDB-SDR-{}",
                    row["attributes"]["WellReportTrackingNumber"]
                        .as_str()
                        .unwrap()
                );
                let (h, m) = records.get(&id).unwrap();
                (Some(id), Some(h.clone()), *m)
            };
            self.observe(id, content, mapped);
        }
    }
    pub(super) fn observe(&mut self, id: Option<String>, content: Option<String>, mapped: bool) {
        if let Some(id) = id {
            let e = self.ids.entry(id).or_insert((content.clone(), 0, mapped));
            if e.0 != content {
                e.0 = None;
            }
            e.1 += 1;
        } else {
            self.anonymous += 1;
        }
    }
    pub(super) fn counts(&self) -> Value {
        let (mut accepted, mut duplicates, mut quarantined, mut mapped) = (0, 0, self.anonymous, 0);
        for (content, n, m) in self.ids.values() {
            if content.is_some() {
                accepted += 1;
                duplicates += n - 1;
                mapped += u64::from(*m);
            } else {
                quarantined += n;
            }
        }
        json!({"acceptedRecords":accepted,"identicalDuplicateRows":duplicates,"quarantinedRows":quarantined,
            "mapEligibleRecords":mapped,"nonmappedRecords":accepted-mapped,"inputRows":accepted+duplicates+quarantined})
    }
    fn summary(&self) -> Value {
        let mut accepted = Vec::new();
        let mut rejected = Vec::new();
        let (mut duplicates, mut quarantined, mut mapped) = (0, self.anonymous, 0);
        for (id, (content, count, is_mapped)) in &self.ids {
            if content.is_some() {
                accepted.push(id);
                duplicates += count - 1;
                mapped += u64::from(*is_mapped);
            } else {
                rejected.push(id);
                quarantined += count;
            }
        }
        json!({"acceptedIds":accepted,"quarantinedIds":rejected,
            "accounting":{"acceptedRecords":accepted.len(),"identicalDuplicateRows":duplicates,
            "quarantinedRows":quarantined,"mapEligibleRecords":mapped,"nonmappedRecords":accepted.len() as u64-mapped,
            "inputRows":accepted.len() as u64+duplicates+quarantined}})
    }
}

fn examine(raw: &[u8], c: &Value, after: u64) -> Result<(Value, Value, u64), String> {
    let input: Value = serde_json::from_slice(raw).map_err(|_| "invalid SDR capture")?;
    let url = public_job_query_url(
        after,
        c["budgets"]["rowsPerPage"].as_u64().unwrap() as usize,
    )?;
    if input["queryUrl"] != url
        || input["fieldLimitationsEvidenceSha256"]
            .as_str()
            .map(str::to_ascii_lowercase)
            != c["dateOmissionEvidenceSha256"]
                .as_str()
                .map(str::to_ascii_lowercase)
    {
        return Err("SDR capture cursor/config mismatch".into());
    }
    let candidate = normalize(raw)?;
    let next = input["pagination"]["objectIds"]
        .as_array()
        .unwrap()
        .last()
        .and_then(Value::as_u64)
        .unwrap_or(after);
    Ok((input, candidate, next))
}

fn rebuild(c: &Value, dir: &Path, s: &Value) -> Result<Reports, String> {
    let pages = s["pages"].as_array().ok_or("invalid SDR pages")?;
    if pages.len() > c["budgets"]["jobPages"].as_u64().unwrap() as usize {
        return Err("SDR page budget exceeded".into());
    }
    let mut reports = Reports::default();
    let mut after = 0;
    let mut exhausted = false;
    let (mut requests, mut response_bytes) = (0, 0);
    for p in pages {
        if exhausted {
            return Err("SDR page follows source exhaustion".into());
        }
        let capture = p["captureSha256"]
            .as_str()
            .ok_or("missing SDR capture hash")?;
        let candidate = p["candidateSha256"]
            .as_str()
            .ok_or("missing SDR candidate hash")?;
        if capture.len() != 64
            || candidate.len() != 64
            || !capture
                .bytes()
                .chain(candidate.bytes())
                .all(|b| b.is_ascii_hexdigit())
            || p["capture"] != format!("sdr-capture-{capture}.json")
            || p["candidate"] != format!("sdr-candidate-{candidate}.json")
        {
            return Err("invalid SDR artifact reference".into());
        }
        let raw = read_capped(&dir.join(p["capture"].as_str().unwrap()), MAX_BYTES)?;
        if hash(&raw) != capture {
            return Err("SDR capture integrity failure".into());
        }
        let (input, value, next) = examine(&raw, c, after)?;
        let output = read_capped(&dir.join(p["candidate"].as_str().unwrap()), 16 * MAX_BYTES)?;
        if hash(&output) != candidate
            || output != encode(&value)?
            || p["afterObjectId"] != after
            || p["lastObjectId"] != next
        {
            return Err("SDR committed page integrity failure".into());
        }
        reports.add(&input, &value);
        after = next;
        exhausted = input["response"]["exceededTransferLimit"] == false;
        let count = p["transportRequests"]
            .as_u64()
            .ok_or("SDR page metrics missing")?;
        let bytes = p["responseBytes"]
            .as_u64()
            .ok_or("SDR page metrics missing")?;
        if count != if c["mode"] == "fixture" { 0 } else { 2 } || bytes > CHARGE {
            return Err("SDR page metrics invalid".into());
        }
        requests += count;
        response_bytes += bytes;
    }
    let reservations = pages.len() as u64 + u64::from(!s["pending"].is_null());
    let b = &c["budgets"];
    if s["knownCommittedTransportRequests"] != requests
        || s["knownCommittedResponseBytes"] != response_bytes
        || s["complete"] != false
        || s["sourceExhausted"] != exhausted
        || s["cursor"] != after
        || s["reservedRequests"] != reservations * 2
        || s["reservedBytes"] != reservations * CHARGE
        || s["reservedRows"] != reservations * b["rowsPerPage"].as_u64().unwrap()
        || reservations > b["jobPages"].as_u64().unwrap()
        || reservations * 2 > b["jobRequests"].as_u64().unwrap()
        || reservations * CHARGE > b["jobBytes"].as_u64().unwrap()
        || reservations * b["rowsPerPage"].as_u64().unwrap() > b["jobRows"].as_u64().unwrap()
        || s["progress"] != reports.summary()
    {
        return Err("SDR checkpoint accounting mismatch".into());
    }
    if !s["pending"].is_null() && (s["pending"]["afterObjectId"] != after || exhausted) {
        return Err("SDR pending cursor mismatch".into());
    }
    Ok(reports)
}

pub(super) fn execute(
    c: &Value,
    dir: &Path,
    status: bool,
    adapter: &mut dyn InventoryAdapter,
) -> Result<Value, String> {
    execute_with_clock(c, dir, status, adapter, &|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    })
}

fn execute_with_clock(
    c: &Value,
    dir: &Path,
    status: bool,
    adapter: &mut dyn InventoryAdapter,
    now: &dyn Fn() -> u64,
) -> Result<Value, String> {
    validate_config(c)?;
    if status && !dir.join("checkpoint-sdr.json").exists() {
        return Ok(json!({"status":"not-started","complete":false}));
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
    let path = dir.join("checkpoint-sdr.json");
    let b = &c["budgets"];
    let fingerprint = hash(&encode(c)?);
    let mut s = if path.exists() {
        let envelope: Value = serde_json::from_slice(&read_capped(&path, MAX_BYTES)?)
            .map_err(|_| "invalid SDR checkpoint")?;
        if envelope["schemaVersion"] != 1
            || envelope["sha256"] != hash(&encode(&envelope["payload"])?)
            || envelope["payload"]["configSha256"] != fingerprint
        {
            return Err("SDR checkpoint integrity/config mismatch".into());
        }
        envelope["payload"].clone()
    } else {
        let started = now();
        json!({"configSha256":fingerprint,"startedAtUnixSeconds":started,
            "expiresAtUnixSeconds":started.checked_add(b["jobSeconds"].as_u64().unwrap()).ok_or("expiry overflow")?,
            "status":"partial","complete":false,"sourceExhausted":false,"cursor":0,"pending":null,"pages":[],
            "reservedRequests":0,"reservedBytes":0,"reservedRows":0,"knownCommittedTransportRequests":0,"knownCommittedResponseBytes":0,"mode":c["mode"],"progress":Reports::default().summary()})
    };
    let mut reports = rebuild(c, dir, &s)?;
    if status {
        return Ok(s);
    }
    if s["blocked"] == true {
        return Err("SDR stopped job; automatic retry refused; status remains available".into());
    }
    let started = s["startedAtUnixSeconds"]
        .as_u64()
        .ok_or("SDR start missing")?;
    let expiry = s["expiresAtUnixSeconds"]
        .as_u64()
        .ok_or("SDR expiry missing")?;
    if expiry
        != started
            .checked_add(b["jobSeconds"].as_u64().unwrap())
            .ok_or("expiry overflow")?
        || now() < started
    {
        return Err("SDR persisted deadline invalid; no renewal".into());
    }
    if !path.exists() {
        persist(&path, &mut s)?;
    }
    let run = Instant::now();
    for _ in 0..b["runPages"].as_u64().unwrap() {
        if s["sourceExhausted"] == true {
            break;
        }
        let replay = !s["pending"].is_null();
        if !replay {
            if now() >= expiry {
                return Err("SDR absolute expiry reached; no renewal".into());
            }
            if run.elapsed().as_secs() >= b["runSeconds"].as_u64().unwrap().saturating_sub(1) {
                break;
            }
            let count = s["pages"].as_array().unwrap().len() as u64 + 1;
            if count > b["jobPages"].as_u64().unwrap()
                || count * 2 > b["jobRequests"].as_u64().unwrap()
                || count * CHARGE > b["jobBytes"].as_u64().unwrap()
                || count * b["rowsPerPage"].as_u64().unwrap() > b["jobRows"].as_u64().unwrap()
            {
                s["status"] = json!("partial-budget-exhausted");
                persist(&path, &mut s)?;
                break;
            }
            s["reservedRequests"] = json!(count * 2);
            s["reservedBytes"] = json!(count * CHARGE);
            s["reservedRows"] = json!(count * b["rowsPerPage"].as_u64().unwrap());
            s["pending"] = json!({"afterObjectId":s["cursor"]});
            persist(&path, &mut s)?;
        }
        let after = s["cursor"].as_u64().unwrap();
        let index = s["pages"].as_array().unwrap().len();
        let cache = dir.join(format!("sdr-cache-{index}"));
        let result = (|| {
            let raw = if replay {
                cached_capture(&cache, c["mode"] == "fixture")
                    .map_err(|_| "SDR interrupted attempt outcome uncertain; no refetch")?
            } else {
                if c["mode"] == "live" {
                    thread::sleep(Duration::from_millis(b["rateMillis"].as_u64().unwrap()));
                }
                let remaining = expiry.saturating_sub(now()).min(
                    b["runSeconds"]
                        .as_u64()
                        .unwrap()
                        .saturating_sub(run.elapsed().as_secs()),
                );
                if remaining < 2 {
                    return Err("SDR deadline reached before request".into());
                }
                let mut item = json!({"_sdrAfterObjectId":after,"_sdrEvidence":c["dateOmissionEvidenceSha256"]});
                if c["mode"] == "fixture" {
                    item["fixture"] = c["fixturePages"][index].clone();
                }
                let raw = adapter.capture(
                    &item,
                    &json!({"bytesPerState":MAX_BYTES,"rowsPerPage":b["rowsPerPage"]}),
                    &cache,
                    remaining,
                )?;
                // Both actual and fixture adapters must durably save the receipt before returning.
                if raw != cached_capture(&cache, c["mode"] == "fixture")? {
                    return Err("SDR adapter/cache mismatch".into());
                }
                raw
            };
            let (input, candidate, next) = examine(&raw, c, after)?;
            let output = encode(&candidate)?;
            if output.len() > 16 * MAX_BYTES {
                return Err("SDR candidate byte cap".into());
            }
            let capture_name = immutable(dir, "sdr-capture", &raw)?;
            let candidate_name = immutable(dir, "sdr-candidate", &output)?;
            let receipt: Value =
                serde_json::from_slice(&read_capped(&cache.join("evidence.json"), MAX_BYTES)?)
                    .map_err(|_| "invalid SDR receipt")?;
            reports.add(&input, &candidate);
            s["pages"].as_array_mut().unwrap().push(json!({"afterObjectId":after,"lastObjectId":next,
                "capture":capture_name,"captureSha256":hash(&raw),"candidate":candidate_name,"candidateSha256":hash(&output),"replayed":replay,
                "transportRequests":receipt["networkRequests"],"responseBytes":receipt["responseBytes"]}));
            s["knownCommittedTransportRequests"] = json!(
                s["knownCommittedTransportRequests"].as_u64().unwrap()
                    + receipt["networkRequests"].as_u64().unwrap()
            );
            s["knownCommittedResponseBytes"] = json!(
                s["knownCommittedResponseBytes"].as_u64().unwrap()
                    + receipt["responseBytes"].as_u64().unwrap()
            );
            s["cursor"] = json!(next);
            s["pending"] = Value::Null;
            s["sourceExhausted"] = json!(input["response"]["exceededTransferLimit"] == false);
            s["status"] = json!(if s["sourceExhausted"] == true {
                "source-pagination-exhausted"
            } else {
                "partial"
            });
            s["progress"] = reports.summary();
            persist(&path, &mut s)
        })();
        if let Err(reason) = result {
            s["blocked"] = json!(true);
            s["status"] = json!("stopped");
            s["stopReason"] = json!(reason);
            // Adapter errors are bounded generic diagnostics; unknown attempts stay charged.
            s["lastTransportMetrics"] = adapter.metrics();
            persist(&path, &mut s)?;
            return Err(reason);
        }
    }
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    struct Scratch(PathBuf);
    impl Scratch {
        fn new() -> Self {
            let p = std::env::temp_dir().join(format!(
                "sdr-jobs-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir_all(&p).unwrap();
            Self(p)
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn config() -> Value {
        json!({"schemaVersion":1,"adapter":"texas-sdr","mode":"fixture","dateOmissionEvidenceSha256":"c".repeat(64),
        "fixturePages":["unused-0","unused-1"],"budgets":{"rowsPerPage":2,"runPages":1,"jobPages":2,"jobRequests":4,"jobBytes":2*CHARGE,"jobRows":4,"jobSeconds":300,"runSeconds":60,"rateMillis":1000}})
    }
    fn page(after: u64, ids: &[(&str, i64)], transfer: bool) -> Vec<u8> {
        let features:Vec<_>=ids.iter().map(|(id,depth)|json!({"attributes":{"WellReportTrackingNumber":id,"LatitudeDD":30.25,"LongitudeDD":-97.75,"ProposedUse":null,"BoreholeDepthFt":depth,"LocationError":-1}})).collect();
        encode(&json!({"schemaVersion":1,"kind":"rainfolk-texas-sdr-public-capture","synthetic":false,
            "sourceUrl":gev_us_groundwater::texas_sdr::SOURCE,"queryUrl":public_job_query_url(after,2).unwrap(),"retrievedAt":"2026-10-26T00:00:00Z",
            "sourceResponseSha256":hash(b"{}"),"sourceSchemaSha256":hash(b"{}"),"fieldLimitations":{"DateOfWellCompletion":"source-query-api-400"},
            "fieldLimitationsEvidenceSha256":"c".repeat(64),"pagination":{"afterObjectId":after,"objectIds":(after+1..=after+ids.len() as u64).collect::<Vec<_>>()},
            "response":{"spatialReference":{"wkid":4326},"spatialReferenceBasis":"source-response; attributes-unverified","exceededTransferLimit":transfer,"features":features}})).unwrap()
    }
    struct Fake {
        pages: Vec<Vec<u8>>,
        calls: usize,
        crash: Option<bool>,
        failure: Option<&'static str>,
    }
    impl Fake {
        fn new() -> Self {
            Self {
                pages: vec![
                    page(0, &[("001", 10), ("002", 20)], true),
                    page(2, &[("001", 10), ("003", 30)], false),
                ],
                calls: 0,
                crash: None,
                failure: None,
            }
        }
    }
    impl InventoryAdapter for Fake {
        fn capture(
            &mut self,
            item: &Value,
            _: &Value,
            cache: &Path,
            _: u64,
        ) -> Result<Vec<u8>, String> {
            self.calls += 1;
            if let Some(reason) = self.failure {
                return Err(reason.into());
            }
            if self.crash == Some(false) {
                self.crash = None;
                panic!("interrupted before durable receipt");
            }
            let index = usize::from(item["_sdrAfterObjectId"] == 2);
            let raw = self.pages[index].clone();
            stage_fixture(cache, &raw)?;
            if self.crash == Some(true) {
                self.crash = None;
                panic!("interrupted after durable receipt");
            }
            Ok(raw)
        }
    }
    fn run(c: &Value, d: &Path, a: &mut Fake, time: u64) -> Result<Value, String> {
        execute_with_clock(c, d, false, a, &|| time)
    }
    #[test]
    fn resumes_source_cursor_deduplicates_ids_and_completed_replay_is_noop() {
        let d = Scratch::new();
        let mut c = config();
        c["dateOmissionEvidenceSha256"] = json!("C".repeat(64));
        let mut a = Fake::new();
        let first = run(&c, &d.0, &mut a, 1000).unwrap();
        assert_eq!(first["cursor"], 2);
        assert_eq!(first["reservedRequests"], 2);
        let second = run(&c, &d.0, &mut a, 1001).unwrap();
        assert_eq!(
            second["progress"]["acceptedIds"],
            json!(["TWDB-SDR-001", "TWDB-SDR-002", "TWDB-SDR-003"])
        );
        assert_eq!(
            second["progress"]["accounting"],
            json!({"acceptedRecords":3,"identicalDuplicateRows":1,"quarantinedRows":0,"mapEligibleRecords":3,"nonmappedRecords":0,"inputRows":4})
        );
        assert_eq!(second["complete"], false);
        assert_eq!(second["sourceExhausted"], true);
        assert_eq!(second["reservedRequests"], 4);
        let bytes = fs::read(d.0.join("checkpoint-sdr.json")).unwrap();
        assert_eq!(run(&c, &d.0, &mut a, 1400).unwrap(), second);
        assert_eq!(a.calls, 2);
        assert_eq!(fs::read(d.0.join("checkpoint-sdr.json")).unwrap(), bytes);
    }
    #[test]
    fn restart_replays_durable_capture_without_refetch_or_double_charge() {
        let d = Scratch::new();
        let c = config();
        let mut a = Fake::new();
        a.crash = Some(true);
        assert!(catch_unwind(AssertUnwindSafe(|| run(&c, &d.0, &mut a, 1000))).is_err());
        let recovered = run(&c, &d.0, &mut a, 1400).unwrap(); // local recovery remains possible after expiry
        assert_eq!(a.calls, 1);
        assert_eq!(recovered["pages"][0]["replayed"], true);
        assert_eq!(recovered["reservedRequests"], 2);
        assert_eq!(recovered["progress"]["accounting"]["acceptedRecords"], 2);
        assert!(run(&c, &d.0, &mut a, 1400).is_err());
        assert_eq!(a.calls, 1);
    }
    #[test]
    fn unknown_interrupted_outcome_never_refetches_and_remains_charged() {
        let d = Scratch::new();
        let c = config();
        let mut a = Fake::new();
        a.crash = Some(false);
        assert!(catch_unwind(AssertUnwindSafe(|| run(&c, &d.0, &mut a, 1000))).is_err());
        assert!(
            run(&c, &d.0, &mut a, 1001)
                .unwrap_err()
                .contains("uncertain")
        );
        assert!(run(&c, &d.0, &mut a, 1002).is_err());
        assert_eq!(a.calls, 1);
        let s = execute_with_clock(&c, &d.0, true, &mut a, &|| 1002).unwrap();
        assert_eq!(s["reservedRequests"], 2);
        assert_eq!(s["blocked"], true);
    }
    #[test]
    fn cross_page_conflicts_and_invalid_versions_quarantine_all_prior_versions() {
        for depth in [99, -1] {
            let d = Scratch::new();
            let c = config();
            let mut a = Fake::new();
            a.pages[1] = page(2, &[("001", depth), ("003", 30)], false);
            run(&c, &d.0, &mut a, 1000).unwrap();
            let s = run(&c, &d.0, &mut a, 1001).unwrap();
            assert_eq!(
                s["progress"]["acceptedIds"],
                json!(["TWDB-SDR-002", "TWDB-SDR-003"])
            );
            assert_eq!(s["progress"]["quarantinedIds"], json!(["TWDB-SDR-001"]));
            assert_eq!(s["progress"]["accounting"]["quarantinedRows"], 2);
            assert_eq!(s["progress"]["accounting"]["inputRows"], 4);
        }
    }
    #[test]
    fn lifetime_request_byte_and_row_limits_do_not_reset_on_resume() {
        for (key, value) in [("jobRequests", 2), ("jobBytes", CHARGE), ("jobRows", 2)] {
            let d = Scratch::new();
            let mut c = config();
            c["budgets"][key] = json!(value);
            let mut a = Fake::new();
            run(&c, &d.0, &mut a, 1000).unwrap();
            let s = run(&c, &d.0, &mut a, 1001).unwrap();
            assert_eq!(s["status"], "partial-budget-exhausted");
            assert_eq!(s["reservedRequests"], 2);
            assert_eq!(a.calls, 1);
        }
    }
    #[test]
    fn expiry_is_absolute_and_clock_rollback_or_changed_config_cannot_renew_it() {
        let d = Scratch::new();
        let c = config();
        let mut a = Fake::new();
        run(&c, &d.0, &mut a, 1000).unwrap();
        for time in [999, 1300, 1400] {
            assert!(run(&c, &d.0, &mut a, time).is_err());
        }
        let mut changed = c.clone();
        changed["budgets"]["jobSeconds"] = json!(301);
        assert!(run(&changed, &d.0, &mut a, 1001).is_err());
        assert_eq!(a.calls, 1);
    }
    #[test]
    fn access_rate_and_generic_errors_stop_without_automatic_retry() {
        for reason in ["access-denial", "rate-limit", "source-error"] {
            let d = Scratch::new();
            let c = config();
            let mut a = Fake::new();
            a.failure = Some(reason);
            assert!(run(&c, &d.0, &mut a, 1000).is_err());
            a.failure = None;
            assert!(run(&c, &d.0, &mut a, 1001).is_err());
            assert_eq!(a.calls, 1);
        }
    }
    #[test]
    fn corrupt_capture_resigned_checkpoint_and_competing_writer_refuse_before_transport() {
        let d = Scratch::new();
        let c = config();
        let mut a = Fake::new();
        let s = run(&c, &d.0, &mut a, 1000).unwrap();
        let p = d.0.join("checkpoint-sdr.json");
        let original = fs::read(&p).unwrap();
        let mut forged = s.clone();
        forged["cursor"] = json!(8);
        persist(&p, &mut forged).unwrap();
        assert!(run(&c, &d.0, &mut a, 1001).is_err());
        fs::write(&p, &original).unwrap();
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .open(d.0.join("runner.lock"))
            .unwrap();
        lock.try_lock().unwrap();
        assert!(run(&c, &d.0, &mut a, 1001).is_err());
        drop(lock);
        fs::write(d.0.join(s["pages"][0]["capture"].as_str().unwrap()), b"{}").unwrap();
        assert!(run(&c, &d.0, &mut a, 1001).is_err());
        assert_eq!(a.calls, 1);
    }
}
