//! Version 2: validated cursor chains and bounded immutable partitions.
use super::*;
use std::collections::{BTreeMap, BTreeSet};
const CHECKPOINT_CAP: usize = 64 * MAX_BYTES;
const PAGE_CHARGE: u64 = 512 * 1024;
#[path = "job_migration.rs"]
mod migration;
#[path = "projected_pages.rs"]
mod projected;
pub(super) use migration::{amend, migrate};
fn page_charge(config: &Value) -> u64 {
    if projected::enabled(config) {
        return 32 * 1024 * 1024;
    }
    if config["budgets"]["rowsPerPage"].as_u64().unwrap_or(0) > 25 {
        4 * 1024 * 1024
    } else {
        PAGE_CHARGE
    }
}

pub(super) fn validate_config(config: &Value) -> Result<(), String> {
    let run_seconds = number(&config["budgets"], "runSeconds", 1, 86400)?;
    let mut old = config.clone();
    old["schemaVersion"] = json!(1);
    old["budgets"]["runSeconds"] = json!(run_seconds.min(300));
    let large = projected::enabled(config);
    let rows = number(
        &config["budgets"],
        "rowsPerPage",
        1,
        if large { 10000 } else { 1000 },
    )?;
    let bytes = number(
        &config["budgets"],
        "bytesPerState",
        1024,
        if large {
            64 * MAX_BYTES as u64
        } else {
            8 * MAX_BYTES as u64
        },
    )?;
    let rate = number(&config["budgets"], "rateMillis", 1000, 3600000)?;
    old["budgets"]["rowsPerPage"] = json!(rows.min(25));
    old["budgets"]["bytesPerState"] = json!(bytes.min(MAX_BYTES as u64));
    old["budgets"]["rateMillis"] = json!(rate.min(10000));
    if large {
        let seconds = number(&config["budgets"], "stateSeconds", 1, 120)?;
        old["budgets"]["stateSeconds"] = json!(seconds.min(90));
        if config["projection"] != crate::page_segments::APPROVED
            || rows != 10000
            || config["budgets"]["pages"] != 1
            || rate < 5000
            || config["stopOnFailure"] != true
        {
            return Err("invalid bounded projected acquisition config".into());
        }
    }
    super::validate_config(&old)?;
    let b = &config["budgets"];
    if !b["jobDiskBytes"].is_null() {
        number(b, "jobDiskBytes", 1024, 1024 * 1024 * 1024 * 1024)?;
    }
    number(b, "runPages", 1, 100000)?;
    number(b, "jobPages", 1, 100000)?;
    number(b, "runBytes", PAGE_CHARGE, 64 * 1024 * 1024 * 1024)?;
    number(b, "jobBytes", PAGE_CHARGE, 64 * 1024 * 1024 * 1024)?;
    if b["runPages"].as_u64() > b["jobPages"].as_u64()
        || b["runBytes"].as_u64() > b["jobBytes"].as_u64()
    {
        return Err("run budget exceeds lifetime job budget".into());
    }
    Ok(())
}
fn first_url(code: &str, limit: u64) -> String {
    format!(
        "https://api.waterdata.usgs.gov/ogcapi/v1/collections/monitoring-locations/items?f=json&limit={limit}&agency_code=USGS&country_code=US&site_type_code=GW&state_code={code}"
    )
}
pub(super) fn decode(value: &str) -> Result<String, String> {
    let mut bytes = Vec::new();
    let input = value.as_bytes();
    let mut i = 0;
    while i < input.len() {
        match input[i] {
            b'%' => {
                let pair = input.get(i + 1..i + 3).ok_or("bad URL escape")?;
                let hex = std::str::from_utf8(pair).map_err(|_| "bad URL escape")?;
                bytes.push(u8::from_str_radix(hex, 16).map_err(|_| "bad URL escape")?);
                i += 3;
            }
            b'+' => {
                bytes.push(b' ');
                i += 1;
            }
            b => {
                bytes.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(bytes).map_err(|_| "invalid URL text".into())
}
// Only URL percent encoding is decoded for equality; the opaque token is never interpreted.
pub(super) fn url_key(url: &str, code: &str, limit: u64) -> Result<String, String> {
    const PREFIX: &str =
        "https://api.waterdata.usgs.gov/ogcapi/v1/collections/monitoring-locations/items?";
    if url.len() > 8192
        || url.bytes().any(|b| !(33..=126).contains(&b))
        || url.contains(['#', '\\'])
    {
        return Err("invalid inventory page URL".into());
    }
    let mut values = BTreeMap::new();
    for part in url
        .strip_prefix(PREFIX)
        .ok_or("inventory page outside fixed origin/path")?
        .split('&')
    {
        let (key, value) = part.split_once('=').ok_or("malformed query")?;
        if values.insert(decode(key)?, decode(value)?).is_some() {
            return Err("duplicate query parameter".into());
        }
    }
    let expected = [
        ("f", "json".to_owned()),
        ("limit", limit.to_string()),
        ("agency_code", "USGS".into()),
        ("country_code", "US".into()),
        ("site_type_code", "GW".into()),
        ("state_code", code.to_owned()),
    ];
    for (key, value) in expected {
        if values.remove(key) != Some(value) {
            return Err("inventory query scope changed".into());
        }
    }
    let cursor = values.remove("cursor");
    if !values.is_empty() {
        return Err("unexpected page query parameter".into());
    }
    match cursor {
        None => Ok("first".into()),
        Some(c)
            if !c.is_empty()
                && c.len() <= 2048
                && !c.chars().any(|v| v.is_control() || v.is_whitespace()) =>
        {
            Ok(format!("cursor:{c}"))
        }
        _ => Err("invalid cursor".into()),
    }
}
#[derive(Default)]
struct Index {
    ids: BTreeSet<String>,
    quarantined: BTreeSet<String>,
    pages: BTreeSet<String>,
    requests: BTreeSet<String>,
    next: Option<String>,
    page_count: u64,
}
struct Delta {
    ids: Vec<String>,
    quarantined: Vec<String>,
    pages: Vec<String>,
    requests: Vec<String>,
    next: Option<String>,
}
fn examine(
    capture: &Value,
    bundle: &Value,
    code: &str,
    limit: u64,
    index: &Index,
) -> Result<Delta, String> {
    examine_projected(capture, bundle, code, limit, index, None)
}
fn examine_projected(
    capture: &Value,
    bundle: &Value,
    code: &str,
    limit: u64,
    index: &Index,
    projection: Option<&str>,
) -> Result<Delta, String> {
    let key_for = |url: &str| match projection {
        Some(p) => crate::page_segments::query(url, code, p),
        None => url_key(url, code, limit),
    };
    let mut delta = Delta {
        ids: Vec::new(),
        quarantined: Vec::new(),
        pages: Vec::new(),
        requests: Vec::new(),
        next: index.next.clone(),
    };
    let acquisitions = capture["acquisitions"]
        .as_array()
        .ok_or("missing acquisition pages")?;
    let mut expected = index.next.clone().unwrap_or_else(|| match projection {
        Some(p) => format!(
            "{}&properties={}",
            first_url(code, limit),
            p.replace(',', "%2C")
        ),
        None => first_url(code, limit),
    });
    if index.page_count > 0 && index.next.is_none() {
        return Err("cannot append after source exhaustion".into());
    }
    for page in acquisitions {
        let url = page["url"].as_str().ok_or("missing request URL")?;
        if url != expected {
            return Err("cursor chain discontinuity".into());
        }
        let key = key_for(url)?;
        if index.requests.contains(&key) || delta.requests.contains(&key) {
            return Err("repeated cursor request".into());
        }
        delta.requests.push(key.clone());
        let body = &page["response"];
        let rows = body["features"].as_array().ok_or("missing source rows")?;
        if body["numberReturned"].as_u64() != Some(rows.len() as u64) {
            return Err("source count mismatch".into());
        }
        let fingerprint = hash(&encode(&body["features"])?);
        if index.pages.contains(&fingerprint) || delta.pages.contains(&fingerprint) {
            return Err("duplicate source page".into());
        }
        delta.pages.push(fingerprint);
        let links = body["links"].as_array().ok_or("missing pagination links")?;
        let selves: Vec<_> = links.iter().filter(|l| l["rel"] == "self").collect();
        if selves.len() != 1 {
            return Err("source self link mismatch".into());
        }
        let self_key = key_for(selves[0]["href"].as_str().ok_or("missing self URL")?)?;
        // USGS live cursor pages advertise the fixed query, without its cursor,
        // as self. Actual request continuity still uses the exact prior next URL.
        if self_key != key && self_key != "first" {
            return Err("source self link mismatch".into());
        }
        let next: Vec<_> = links.iter().filter(|l| l["rel"] == "next").collect();
        if next.len() > 1 {
            return Err("ambiguous source pagination".into());
        }
        delta.next = next
            .first()
            .map(|l| {
                l["href"]
                    .as_str()
                    .ok_or("missing next URL")
                    .map(str::to_owned)
            })
            .transpose()?;
        if let Some(next) = &delta.next {
            let token = key_for(next)?;
            if token == "first"
                || index.requests.contains(&token)
                || delta.requests.contains(&token)
            {
                return Err("pagination cursor loop".into());
            }
            if rows.is_empty() {
                return Err("empty nonterminal page".into());
            }
            expected = next.clone();
        } else {
            expected = String::new();
        }
    }
    let rows: usize = acquisitions
        .iter()
        .map(|p| p["response"]["features"].as_array().unwrap().len())
        .sum();
    let records = bundle["inventory"]["records"]
        .as_array()
        .ok_or("missing inventory")?;
    let quarantined = bundle["layerPack"]["coverage"]["quarantine"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    if rows != records.len() + quarantined.len() {
        return Err("duplicate IDs within partition".into());
    }
    for record in &quarantined {
        let id = record["sourceId"]
            .as_str()
            .ok_or("missing quarantine ID")?
            .to_owned();
        if index.ids.contains(&id)
            || index.quarantined.contains(&id)
            || delta.quarantined.contains(&id)
        {
            return Err("duplicate quarantined source ID across partitions".into());
        }
        delta.quarantined.push(id);
    }
    for record in records {
        let id = record["id"].as_str().ok_or("missing native ID")?.to_owned();
        if index.ids.contains(&id)
            || index.quarantined.contains(&id)
            || delta.quarantined.contains(&id)
            || delta.ids.contains(&id)
        {
            return Err("duplicate source ID across partitions".into());
        }
        delta.ids.push(id);
    }
    Ok(delta)
}
fn apply(index: &mut Index, delta: Delta) {
    index.ids.extend(delta.ids);
    index.quarantined.extend(delta.quarantined);
    index.page_count += delta.pages.len() as u64;
    index.pages.extend(delta.pages);
    index.requests.extend(delta.requests);
    index.next = delta.next;
}
fn initial(config: &Value) -> Result<Value, String> {
    let states:serde_json::Map<String,Value>=config["states"].as_array().unwrap().iter().map(|v|(v["state"].as_str().unwrap().into(),json!({"status":"pending","attempts":0,"accepted":0,"pages":0,"nextPage":null,"partitions":[]}))).collect();
    Ok(
        json!({"schemaVersion":2,"configSha256":hash(&encode(config)?),"generation":0,"states":states,"reservations":[],"chargedPages":0,"chargedBytes":0}),
    )
}
fn load(config: &Value, dir: &Path) -> Result<(Value, BTreeMap<String, Index>), String> {
    let baseline = projected::baseline(config)?;
    load_with_baseline(config, dir, &baseline)
}
fn load_with_baseline(
    config: &Value,
    dir: &Path,
    baseline: &BTreeMap<String, String>,
) -> Result<(Value, BTreeMap<String, Index>), String> {
    let path = dir.join("checkpoint-v2.json");
    let state = if path.exists() {
        let envelope: Value = serde_json::from_slice(&read_capped(&path, CHECKPOINT_CAP)?)
            .map_err(|e| e.to_string())?;
        let state = &envelope["payload"];
        if envelope["schemaVersion"] != 1
            || state["schemaVersion"] != 2
            || envelope["sha256"] != hash(&encode(state)?)
            || state["configSha256"] != hash(&encode(config)?)
        {
            return Err("checkpoint integrity/config mismatch".into());
        }
        state.clone()
    } else {
        initial(config)?
    };
    if state["states"]
        .as_object()
        .ok_or("missing state table")?
        .len()
        != config["states"].as_array().unwrap().len()
    {
        return Err("state queue mismatch".into());
    }
    let charge = page_charge(config);
    let ledger = state["reservations"]
        .as_array()
        .ok_or("missing budget ledger")?;
    let mut charged_pages = 0;
    let mut charged_bytes = 0;
    let mut attempts = BTreeMap::<String, u64>::new();
    for entry in ledger {
        let code = entry["state"].as_str().ok_or("missing reservation state")?;
        if state["states"].get(code).is_none() {
            return Err("unknown reservation state".into());
        }
        let pages = number(entry, "pages", 1, 2)?;
        let bytes = number(entry, "bytes", charge, 2 * charge)?;
        if bytes != pages * charge {
            return Err("invalid byte reservation".into());
        }
        charged_pages += pages;
        charged_bytes += bytes;
        *attempts.entry(code.into()).or_default() += 1;
    }
    if state["chargedPages"] != charged_pages
        || state["chargedBytes"] != charged_bytes
        || charged_pages > config["budgets"]["jobPages"].as_u64().unwrap()
        || charged_bytes > config["budgets"]["jobBytes"].as_u64().unwrap()
    {
        return Err("job budget ledger mismatch".into());
    }
    let limit = config["budgets"]["rowsPerPage"].as_u64().unwrap();
    let mut indices = BTreeMap::new();
    for item in config["states"].as_array().unwrap() {
        let code = item["state"].as_str().unwrap();
        let record = &state["states"][code];
        if !["pending", "running", "partial", "completed", "failed"]
            .iter()
            .any(|s| record["status"] == *s)
            || record["attempts"].as_u64() != Some(*attempts.get(code).unwrap_or(&0))
        {
            return Err("checkpoint status/attempt mismatch".into());
        }
        let mut index = Index::default();
        for partition in record["partitions"]
            .as_array()
            .ok_or("missing partitions")?
        {
            if projected::enabled(config) {
                let (capture, summary) =
                    projected::restore(config, dir, code, partition, baseline)?;
                let delta = examine_projected(
                    &capture,
                    &summary,
                    code,
                    limit,
                    &index,
                    config["projection"].as_str(),
                )?;
                apply(&mut index, delta);
                continue;
            }
            let digest = partition["candidateSha256"]
                .as_str()
                .ok_or("missing candidate hash")?;
            if digest.len() != 64
                || !digest.bytes().all(|b| b.is_ascii_hexdigit())
                || partition["candidate"] != format!("{code}-candidate-{digest}.json")
            {
                return Err("invalid candidate reference".into());
            }
            let bytes = read_capped(
                &dir.join(partition["candidate"].as_str().unwrap()),
                16 * MAX_BYTES,
            )?;
            if hash(&bytes) != digest {
                return Err("candidate hash mismatch".into());
            }
            let bundle: Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
            gev_us_groundwater::validate_bundle(&bundle)?;
            let source_hash = bundle["manifest"]["input"]["sha256"]
                .as_str()
                .ok_or("missing capture hash")?;
            let name = format!("{code}-capture-{source_hash}.json");
            if partition["capture"] != name {
                return Err("capture reference mismatch".into());
            }
            let raw = read_capped(&dir.join(name), 8 * MAX_BYTES)?;
            if hash(&raw) != source_hash {
                return Err("capture hash mismatch".into());
            }
            let capture: Value = serde_json::from_slice(&raw).map_err(|e| e.to_string())?;
            if gev_us_groundwater::inventory_bundle(&raw, code)? != bundle {
                return Err("candidate/source reconciliation failed".into());
            }
            let delta = examine(&capture, &bundle, code, limit, &index)?;
            if partition["rows"] != delta.ids.len()
                || partition["pages"] != delta.pages.len()
                || partition["nextPage"] != json!(delta.next)
            {
                return Err("partition accounting mismatch".into());
            }
            apply(&mut index, delta);
        }
        if record["accepted"] != index.ids.len()
            || (record.get("quarantined").is_some()
                && record["quarantined"] != index.quarantined.len())
            || record["pages"] != index.page_count
            || record["nextPage"] != json!(index.next)
        {
            return Err("checkpoint cursor/count reconciliation failed".into());
        }
        if record["status"] == "completed" && (index.page_count == 0 || index.next.is_some()) {
            return Err("completed before source exhausted".into());
        }
        if record["status"] == "partial" && index.next.is_none() {
            return Err("partial without continuation token".into());
        }
        indices.insert(code.into(), index);
    }
    let mut unique_ids = BTreeSet::new();
    for index in indices.values() {
        for id in index.ids.iter().chain(index.quarantined.iter()) {
            if !unique_ids.insert(id) {
                return Err("duplicate source ID across state partitions".into());
            }
        }
    }
    Ok((state, indices))
}
fn persist(path: &Path, state: &mut Value) -> Result<(), String> {
    if encode(state)?.len() > CHECKPOINT_CAP - 256 {
        return Err("checkpoint metadata budget reached".into());
    }
    super::save(path, state)
}
fn disk_used(dir: &Path) -> Result<u64, String> {
    let mut bytes = 0u64;
    for entry in fs::read_dir(dir).map_err(|e| e.to_string())? {
        let path = entry.map_err(|e| e.to_string())?.path();
        let metadata = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
        if metadata.file_type().is_symlink() {
            return Err("job directory contains a link; disk scope refused".into());
        }
        bytes = bytes
            .checked_add(if metadata.is_dir() {
                disk_used(&path)?
            } else {
                metadata.len()
            })
            .ok_or("disk byte overflow")?;
    }
    Ok(bytes)
}
pub(super) fn execute(
    config: &Value,
    dir: &Path,
    status: bool,
    adapter: &mut dyn InventoryAdapter,
) -> Result<Value, String> {
    execute_with_clock(config, dir, status, adapter, &|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    })
}
fn execute_with_clock(
    config: &Value,
    dir: &Path,
    status: bool,
    adapter: &mut dyn InventoryAdapter,
    now: &dyn Fn() -> u64,
) -> Result<Value, String> {
    validate_config(config)?;
    if status {
        return load(config, dir).map(|(state, _)| state);
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
    let baseline = projected::baseline(config)?;
    let (mut state, mut indices) = load_with_baseline(config, dir, &baseline)?;
    let mut global_ids = BTreeMap::<String, String>::new();
    for (code, index) in &indices {
        for id in index.ids.iter().chain(index.quarantined.iter()) {
            if global_ids.insert(id.clone(), code.clone()).is_some() {
                return Err("duplicate source ID across state partitions".into());
            }
        }
    }
    let path = dir.join("checkpoint-v2.json");
    if !path.exists() {
        let started = now();
        let expiry = started
            .checked_add(config["budgets"]["runSeconds"].as_u64().unwrap())
            .ok_or("job expiry overflow")?;
        state["startedAtUnixSeconds"] = json!(started);
        state["expiresAtUnixSeconds"] =
            json!(expiry.min(config["absoluteExpiry"].as_u64().unwrap_or(u64::MAX)));
        persist(&path, &mut state)?;
    }
    let expiry = match (state["startedAtUnixSeconds"].as_u64(), state["expiresAtUnixSeconds"].as_u64()) {
        (Some(started), Some(expiry)) if expiry > started && expiry - started <= config["budgets"]["runSeconds"].as_u64().unwrap() => {
            if now() < started { return Err("clock precedes persisted job start; refusing continuation".into()); }
            if now() >= expiry { return Err("absolute job expiry reached; no automatic renewal; status remains available".into()); }
            expiry
        }
        (None, None) if config["mode"] != "live" => u64::MAX,
        _ => return Err("legacy or invalid checkpoint expiry: network continuation refused; offline status remains available; explicit bounded migration required".into()),
    };
    if state["networkBlocked"] == true || state["quotaBlocked"] == true {
        return Err("previous access or quota stop; automatic resume refused".into());
    }
    let b = &config["budgets"];
    let start = Instant::now();
    let charge = page_charge(config);
    let mut run_pages = 0;
    let mut run_bytes = 0;
    let mut report = json!({"networkRequests":0,"cacheHits":0,"rawResponseBytes":0,"committedPartitions":0,"unknownMetricAttempts":0});
    let items = config["states"].as_array().unwrap();
    let first = state["reservations"]
        .as_array()
        .unwrap()
        .last()
        .and_then(|last| items.iter().position(|item| item["state"] == last["state"]))
        .map(|n| (n + 1) % items.len())
        .unwrap_or(0);
    let mut failed_this_run = BTreeSet::new();
    'queue: loop {
        let mut progressed = false;
        for offset in 0..items.len() {
            let item = &items[(first + offset) % items.len()];
            let code = item["state"].as_str().unwrap();
            if state["states"][code]["status"] == "completed" || failed_this_run.contains(code) {
                continue;
            }
            loop {
                if state["states"][code]["blocked"] == true {
                    break;
                }
                let planned = b["pages"]
                    .as_u64()
                    .unwrap()
                    .min(b["runPages"].as_u64().unwrap().saturating_sub(run_pages))
                    .min(
                        b["jobPages"]
                            .as_u64()
                            .unwrap()
                            .saturating_sub(state["chargedPages"].as_u64().unwrap()),
                    )
                    .min(b["runBytes"].as_u64().unwrap().saturating_sub(run_bytes) / charge)
                    .min(
                        b["jobBytes"]
                            .as_u64()
                            .unwrap()
                            .saturating_sub(state["chargedBytes"].as_u64().unwrap())
                            / charge,
                    );
                let bytes = planned * charge;
                if planned == 0
                    || bytes > b["runBytes"].as_u64().unwrap().saturating_sub(run_bytes)
                    || bytes
                        > b["jobBytes"]
                            .as_u64()
                            .unwrap()
                            .saturating_sub(state["chargedBytes"].as_u64().unwrap())
                    || start.elapsed() >= Duration::from_secs(b["runSeconds"].as_u64().unwrap())
                    || now() >= expiry
                {
                    break 'queue;
                }
                let disk_limit = b["jobDiskBytes"]
                    .as_u64()
                    .unwrap_or(b["jobBytes"].as_u64().unwrap() * 32);
                let reserve_disk = if projected::enabled(config) {
                    274 * 1024 * 1024
                } else {
                    b["bytesPerState"].as_u64().unwrap() * 17
                        + bytes * 2
                        + encode(&state)?.len() as u64 * 2
                        + 65536
                };
                if disk_used(dir)? + reserve_disk > disk_limit {
                    report["stopReason"] = json!("disk budget");
                    break 'queue;
                }
                if config["mode"] == "live" {
                    let delay = Duration::from_millis(b["rateMillis"].as_u64().unwrap());
                    if start.elapsed() + delay
                        >= Duration::from_secs(b["runSeconds"].as_u64().unwrap())
                        || expiry.saturating_sub(now()) <= delay.as_secs()
                    {
                        break 'queue;
                    }
                    thread::sleep(delay);
                }
                let remaining = b["runSeconds"]
                    .as_u64()
                    .unwrap()
                    .saturating_sub(start.elapsed().as_secs())
                    .min(expiry.saturating_sub(now()));
                if remaining == 0 {
                    break 'queue;
                }
                let index = indices.get_mut(code).unwrap();
                let mut request = item.clone();
                request["_partitionIndex"] = json!(
                    state["states"][code]["partitions"]
                        .as_array()
                        .unwrap()
                        .len()
                );
                if let Some(url) = &index.next {
                    request["_startUrl"] = json!(url);
                }
                state["chargedPages"] = json!(state["chargedPages"].as_u64().unwrap() + planned);
                state["chargedBytes"] = json!(state["chargedBytes"].as_u64().unwrap() + bytes);
                state["reservations"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!({"state":code,"pages":planned,"bytes":bytes}));
                state["states"][code]["attempts"] =
                    json!(state["states"][code]["attempts"].as_u64().unwrap() + 1);
                state["states"][code]["status"] = json!("running");
                persist(&path, &mut state)?;
                run_pages += planned;
                run_bytes += bytes;
                let mut partition_budget = b.clone();
                partition_budget["pages"] = json!(planned);
                partition_budget["_pageBytes"] = json!(charge);
                if projected::enabled(config) {
                    partition_budget["projection"] = config["projection"].clone();
                }
                let attempt = (|| -> Result<(Delta, Value), String> {
                    let remaining = remaining.min(expiry.saturating_sub(now()));
                    if remaining == 0 {
                        return Err("absolute job expiry reached before acquisition".into());
                    }
                    let raw = adapter.capture(
                        &request,
                        &partition_budget,
                        &dir.join(format!("cache-{code}")),
                        remaining.min(b["stateSeconds"].as_u64().unwrap()),
                    )?;
                    let capture: Value = serde_json::from_slice(&raw).map_err(|e| e.to_string())?;
                    let pages = capture["acquisitions"].as_array().ok_or("missing pages")?;
                    if raw.len() > b["bytesPerState"].as_u64().unwrap() as usize
                        || pages.is_empty()
                        || pages.len() > planned as usize
                        || pages.iter().any(|p| {
                            p["response"]["features"].as_array().is_none_or(|rows| {
                                rows.len() > b["rowsPerPage"].as_u64().unwrap() as usize
                            })
                        })
                    {
                        return Err("partition budget exceeded".into());
                    }
                    if projected::enabled(config) {
                        let (summary, part) =
                            projected::admit(config, dir, code, &capture, &baseline)?;
                        let delta = examine_projected(
                            &capture,
                            &summary,
                            code,
                            b["rowsPerPage"].as_u64().unwrap(),
                            index,
                            config["projection"].as_str(),
                        )?;
                        if delta
                            .ids
                            .iter()
                            .chain(delta.quarantined.iter())
                            .any(|id| global_ids.get(id).is_some_and(|owner| owner != code))
                        {
                            return Err("duplicate source ID across state partitions".into());
                        }
                        return Ok((delta, part));
                    }
                    let bundle = gev_us_groundwater::inventory_bundle(&raw, code)?;
                    let delta = examine(
                        &capture,
                        &bundle,
                        code,
                        b["rowsPerPage"].as_u64().unwrap(),
                        index,
                    )?;
                    if delta
                        .ids
                        .iter()
                        .chain(delta.quarantined.iter())
                        .any(|id| global_ids.get(id).is_some_and(|owner| owner != code))
                    {
                        return Err("duplicate source ID across state partitions".into());
                    }
                    let capture_name = immutable(dir, &format!("{code}-capture"), &raw)?;
                    let encoded = encode(&bundle)?;
                    if encoded.len() > 16 * MAX_BYTES {
                        return Err("candidate byte cap".into());
                    }
                    let candidate = immutable(dir, &format!("{code}-candidate"), &encoded)?;
                    let part = json!({"capture":capture_name,"candidate":candidate,"candidateSha256":hash(&encoded),"rows":delta.ids.len(),"pages":delta.pages.len(),"nextPage":delta.next});
                    Ok((delta, part))
                })();
                let metrics = adapter.metrics();
                if projected::enabled(config) {
                    for key in ["networkRequests", "cacheHits", "rawResponseBytes"] {
                        state["knownTransportTotals"][key] = json!(
                            state["knownTransportTotals"][key].as_u64().unwrap_or(0)
                                + metrics[key].as_u64().unwrap_or(0)
                        );
                    }
                    if metrics.is_null() {
                        state["unknownTransportAttempts"] =
                            json!(state["unknownTransportAttempts"].as_u64().unwrap_or(0) + 1);
                    }
                }
                for key in ["networkRequests", "cacheHits", "rawResponseBytes"] {
                    report[key] =
                        json!(report[key].as_u64().unwrap() + metrics[key].as_u64().unwrap_or(0));
                }
                if config["mode"] == "live" && metrics.is_null() {
                    report["unknownMetricAttempts"] =
                        json!(report["unknownMetricAttempts"].as_u64().unwrap() + 1);
                }
                if let Some(quota) = metrics["quotaHeaders"]
                    .as_array()
                    .and_then(|headers| headers.last())
                {
                    state["lastQuotaHeaders"] = quota.clone();
                    if quota["X-RateLimit-Remaining"]
                        .as_str()
                        .and_then(|s| s.parse::<u64>().ok())
                        .is_some_and(|left| {
                            if projected::enabled(config) {
                                left < 100
                            } else {
                                left <= 5
                            }
                        })
                    {
                        state["quotaBlocked"] = json!(true);
                    }
                }
                match attempt {
                    Ok((delta, part)) => {
                        progressed = true;
                        for id in delta.ids.iter().chain(delta.quarantined.iter()) {
                            global_ids.insert(id.clone(), code.to_owned());
                        }
                        apply(index, delta);
                        let record = &mut state["states"][code];
                        record["partitions"].as_array_mut().unwrap().push(part);
                        record["accepted"] = json!(index.ids.len());
                        record["quarantined"] = json!(index.quarantined.len());
                        record["pages"] = json!(index.page_count);
                        record["observations"] = json!(0);
                        record["nextPage"] = json!(index.next);
                        record["status"] = json!(if index.next.is_none() {
                            "completed"
                        } else {
                            "partial"
                        });
                        record["error"] = Value::Null;
                        report["committedPartitions"] =
                            json!(report["committedPartitions"].as_u64().unwrap() + 1);
                        persist(&path, &mut state)?;
                        if state["quotaBlocked"] == true {
                            break 'queue;
                        }
                        if index.next.is_none() {
                            break;
                        }
                    }
                    Err(error) => {
                        let blocked =
                            ["403", "401", "429", "Permission denied", "Access is denied"]
                                .iter()
                                .any(|s| error.contains(s));
                        failed_this_run.insert(code.to_owned());
                        state["states"][code]["status"] = json!("failed");
                        state["states"][code]["error"] = json!(error);
                        state["states"][code]["blocked"] = json!(blocked);
                        if blocked {
                            state["networkBlocked"] = json!(true);
                        }
                        persist(&path, &mut state)?;
                        if blocked
                            || state["quotaBlocked"] == true
                            || config["stopOnFailure"] == true
                        {
                            break 'queue;
                        }
                        break;
                    }
                }
                break; // one partition per state per round
            }
        }
        if !progressed {
            break;
        }
    }
    if !path.exists() {
        persist(&path, &mut state)?;
    }
    report["reservedPages"] = json!(run_pages);
    report["reservedBytes"] = json!(run_bytes);
    report["elapsedMillis"] = json!(start.elapsed().as_millis());
    if projected::enabled(config) {
        let mut raw_rows = 0;
        let mut accepted = 0;
        let mut quarantined = 0;
        let mut overlap = 0;
        let mut changed = 0;
        let mut new = 0;
        for record in state["states"].as_object().unwrap().values() {
            accepted += record["accepted"].as_u64().unwrap_or(0);
            quarantined += record["quarantined"].as_u64().unwrap_or(0);
            for part in record["partitions"].as_array().unwrap() {
                raw_rows += part["rawRows"].as_u64().unwrap_or(0);
                overlap += part["overlapAccepted"].as_u64().unwrap_or(0);
                changed += part["changedOverlap"].as_u64().unwrap_or(0);
                new += part["newAccepted"].as_u64().unwrap_or(0);
            }
        }
        report["coverage"] = json!({"rawRows":raw_rows,"streamAccepted":accepted,"quarantined":quarantined,
            "overlapAccepted":overlap,"changedOverlap":changed,"newAccepted":new,
            "baselineAccepted":baseline.len(),"combinedAccepted":baseline.len() as u64+new});
        report["knownAggregateUsage"] = json!({"priorNationalReservations":config["priorUsage"]["nationalReservations"],
            "priorKnownNationalRequests":config["priorUsage"]["nationalRequests"],
            "benchmarkRequests":config["priorUsage"]["benchmarkRequests"],"credentialVerificationRequests":config["priorUsage"]["credentialVerificationRequests"],
            "stream":state["knownTransportTotals"],"unknownTransportAttempts":state["unknownTransportAttempts"],
            "scope":"national streams plus named benchmark/key check; earlier pilots excluded"});
        if report["stopReason"].is_null() {
            report["stopReason"] = json!(if state["networkBlocked"] == true {
                "access/failure"
            } else if state["quotaBlocked"] == true {
                "low quota"
            } else if now() >= expiry {
                "absolute expiry"
            } else if run_pages >= b["runPages"].as_u64().unwrap() {
                "reservation cap"
            } else {
                "queue exhausted or validation/time stop"
            });
        }
        state["latestRunReport"] = report.clone();
        persist(&path, &mut state)?;
    }
    state["runReport"] = report;
    Ok(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn migration_recovers_reserved_cache_without_new_charges() {
        let source = scratch();
        let cfg = config(1);
        let mut adapter = fixture();
        adapter.parts.clear();
        let failed = execute_with_clock(&cfg, &source, false, &mut adapter, &|| 1000).unwrap();
        assert_eq!(failed["states"]["36"]["status"], "failed");
        let raw = null_geometry(&capture(None, Some("A"), &[1, 2]), 0);
        let parsed: Value = serde_json::from_slice(&raw).unwrap();
        let page = &parsed["acquisitions"][0];
        let cache = source.join("cache-36");
        fs::create_dir_all(&cache).unwrap();
        let body = serde_json::to_string(&page["response"]).unwrap();
        fs::write(cache.join("page.json"),encode(&json!({"url":page["url"],"retrievedAt":page["retrievedAt"],"body":body,"sha256":hash(body.as_bytes())})).unwrap()).unwrap();
        let recovery = source.with_extension("recovery");
        fs::create_dir_all(&recovery).unwrap();
        fs::write(recovery.join("36-capture.json"), &raw).unwrap();
        let target = source.with_extension("migrated");
        let recovered = migrate(&cfg, &source, &cfg, &target, 2020, Some(&recovery), 2000).unwrap();
        assert_eq!(recovered["chargedPages"], failed["chargedPages"]);
        assert_eq!(recovered["chargedBytes"], failed["chargedBytes"]);
        assert_eq!(recovered["states"]["36"]["accepted"], 1);
        assert_eq!(recovered["states"]["36"]["quarantined"], 1);
        assert_eq!(recovered["states"]["36"]["status"], "partial");
    }
    #[test]
    fn explicit_migration_preserves_source_ledger_cursor_and_rejects_reuse() {
        let source = scratch();
        let cfg = config(1);
        let mut adapter = fixture();
        execute_with_clock(&cfg, &source, false, &mut adapter, &|| 1000).unwrap();
        let original = fs::read(source.join("checkpoint-v2.json")).unwrap();
        let target = source.with_extension("continuation");
        let mut new = cfg.clone();
        new["budgets"]["jobPages"] = json!(30);
        let state = migrate(&cfg, &source, &new, &target, 2020, None, 2000).unwrap();
        assert_eq!(state["chargedPages"], 1);
        assert_eq!(
            state["states"]["36"]["nextPage"],
            load(&cfg, &source).unwrap().0["states"]["36"]["nextPage"]
        );
        assert_eq!(
            state["continuationParent"]["checkpointSha256"],
            hash(&original)
        );
        assert_eq!(
            fs::read(source.join("checkpoint-v2.json")).unwrap(),
            original
        );
        assert!(migrate(&cfg, &source, &new, &target, 2020, None, 2000).is_err());
        assert!(execute_with_clock(&new, &target, false, &mut adapter, &|| 2020).is_err());
    }
    #[test]
    fn absolute_expiry_survives_repeated_resume_and_preserves_ledger() {
        let dir = scratch();
        let cfg = config(1);
        let clock = std::cell::Cell::new(1000_u64);
        let mut source = fixture();
        let first = execute_with_clock(&cfg, &dir, false, &mut source, &|| clock.get()).unwrap();
        assert_eq!(first["expiresAtUnixSeconds"], 1030);
        clock.set(1010);
        let second = execute_with_clock(&cfg, &dir, false, &mut source, &|| clock.get()).unwrap();
        assert_eq!(second["expiresAtUnixSeconds"], 1030);
        assert_eq!(second["chargedPages"], 2);
        assert_eq!(
            second["chargedBytes"].as_u64().unwrap(),
            first["chargedBytes"].as_u64().unwrap() * 2
        );
        let path = dir.join("checkpoint-v2.json");
        let bytes = fs::read(&path).unwrap();
        let calls = source.calls;
        for time in [1030, 1031, 5000] {
            clock.set(time);
            assert!(
                execute_with_clock(&cfg, &dir, false, &mut source, &|| clock.get())
                    .unwrap_err()
                    .contains("expiry")
            );
            assert_eq!(fs::read(&path).unwrap(), bytes);
            assert_eq!(source.calls, calls);
        }
        execute_with_clock(&cfg, &dir, true, &mut source, &|| clock.get()).unwrap();
    }
    #[test]
    fn legacy_live_checkpoint_remains_readable_but_cannot_continue() {
        let dir = scratch();
        let mut cfg = config(1);
        let mut source = fixture();
        let mut state = execute_with_clock(&cfg, &dir, false, &mut source, &|| 1000).unwrap();
        cfg["mode"] = json!("live");
        state["configSha256"] = json!(hash(&encode(&cfg).unwrap()));
        state
            .as_object_mut()
            .unwrap()
            .remove("startedAtUnixSeconds");
        state
            .as_object_mut()
            .unwrap()
            .remove("expiresAtUnixSeconds");
        let path = dir.join("checkpoint-v2.json");
        persist(&path, &mut state).unwrap();
        let bytes = fs::read(&path).unwrap();
        let calls = source.calls;
        execute_with_clock(&cfg, &dir, true, &mut source, &|| 2000).unwrap();
        assert!(
            execute_with_clock(&cfg, &dir, false, &mut source, &|| 2000)
                .unwrap_err()
                .contains("legacy")
        );
        assert_eq!(bytes, fs::read(&path).unwrap());
        assert_eq!(calls, source.calls);
    }
    fn null_geometry(bytes: &[u8], row: usize) -> Vec<u8> {
        let mut value: Value = serde_json::from_slice(bytes).unwrap();
        value["acquisitions"][0]["response"]["features"][row]["geometry"] = Value::Null;
        encode(&value).unwrap()
    }
    #[test]
    fn quarantine_mixed_all_invalid_continuation_and_replay() {
        let dir = scratch();
        let cfg = config(1);
        let mut source = fixture();
        source.parts[0] = null_geometry(&source.parts[0], 0);
        source.parts[1] = null_geometry(&null_geometry(&source.parts[1], 0), 1);
        let first = execute(&cfg, &dir, false, &mut source).unwrap();
        assert_eq!(first["states"]["36"]["accepted"], 1);
        assert_eq!(first["states"]["36"]["quarantined"], 1);
        assert_eq!(first["states"]["36"]["status"], "partial");
        let second = execute(&cfg, &dir, false, &mut source).unwrap();
        assert_eq!(second["states"]["36"]["accepted"], 1);
        assert_eq!(second["states"]["36"]["quarantined"], 3);
        assert_eq!(second["states"]["36"]["status"], "partial");
        let final_state = execute(&cfg, &dir, false, &mut source).unwrap();
        assert_eq!(final_state["states"]["36"]["accepted"], 2);
        assert_eq!(final_state["states"]["36"]["status"], "completed");
        let calls = source.calls;
        execute(&cfg, &dir, true, &mut source).unwrap();
        execute(&cfg, &dir, false, &mut source).unwrap();
        assert_eq!(source.calls, calls);
    }
    #[test]
    fn quarantine_ids_cannot_reappear_in_later_pages() {
        let dir = scratch();
        let cfg = config(1);
        let mut source = fixture();
        source.parts[0] = null_geometry(&source.parts[0], 0);
        source.parts[1] = capture(Some("A"), None, &[1, 3]);
        execute(&cfg, &dir, false, &mut source).unwrap();
        let failed = execute(&cfg, &dir, false, &mut source).unwrap();
        assert_eq!(failed["states"]["36"]["status"], "failed");
        assert_eq!(failed["states"]["36"]["accepted"], 1);
        assert_eq!(failed["states"]["36"]["quarantined"], 1);
    }
    fn scratch() -> PathBuf {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../.temp-gev")
            .join(format!(
                "parts-test-{}",
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }
    fn config(run_pages: u64) -> Value {
        json!({"schemaVersion":2,"adapter":"usgs-monitoring-locations","mode":"fixture","states":[{"state":"36"}],"budgets":{"pages":1,"rowsPerPage":2,"bytesPerState":1048576,"stateSeconds":10,"runSeconds":30,"rateMillis":1000,"runPages":run_pages,"runBytes":10485760,"jobPages":20,"jobBytes":10485760,"jobDiskBytes":1073741824}})
    }
    fn capture(start: Option<&str>, next: Option<&str>, ids: &[u64]) -> Vec<u8> {
        let first = first_url("36", 2);
        let url = start
            .map(|token| format!("{first}&cursor={token}"))
            .unwrap_or(first.clone());
        let original: Value = serde_json::from_slice(include_bytes!(
            "../../../fixtures/us-groundwater/usgs-inventory-pilot/capture.json"
        ))
        .unwrap();
        let mut rows = Vec::new();
        for id in ids {
            let mut row = original["acquisitions"][0]["response"]["features"][0].clone();
            let id = format!("USGS-{id:015}");
            row["id"] = json!(id);
            row["properties"]["id"] = json!(id);
            row["properties"]["monitoring_location_number"] = json!(id.trim_start_matches("USGS-"));
            row["properties"]["monitoring_location_name"] = json!("Synthetic continuation test");
            rows.push(row);
        }
        let mut links = vec![json!({"rel":"self","href":url})];
        if let Some(token) = next {
            links.push(json!({"rel":"next","href":format!("{first}&cursor={token}")}));
        }
        encode(&json!({"schemaVersion":1,"acquisitions":[{"collection":"monitoring-locations","url":url,"retrievedAt":"2026-10-25T00:00:00Z","response":{"type":"FeatureCollection","numberReturned":rows.len(),"features":rows,"links":links}}]})).unwrap()
    }
    struct Fixture {
        parts: Vec<Vec<u8>>,
        calls: usize,
        panic_at: Option<usize>,
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
            if item["state"] == "55" {
                return Err("synthetic state failure".into());
            }
            let n = item["_partitionIndex"].as_u64().unwrap() as usize;
            if self.panic_at == Some(n) {
                self.panic_at = None;
                panic!("simulated cancellation after reservation");
            }
            self.parts
                .get(n)
                .cloned()
                .ok_or_else(|| "missing fixture".into())
        }
    }
    fn fixture() -> Fixture {
        Fixture {
            parts: vec![
                capture(None, Some("A"), &[1, 2]),
                capture(Some("A"), Some("B"), &[3, 4]),
                capture(Some("B"), None, &[5]),
            ],
            calls: 0,
            panic_at: None,
        }
    }
    #[test]
    fn automatic_continuation_exhausts_source_in_one_invocation() {
        let dir = scratch();
        let c = config(10);
        let mut f = fixture();
        let result = execute(&c, &dir, false, &mut f).unwrap();
        assert_eq!(f.calls, 3);
        assert_eq!(result["states"]["36"]["status"], "completed");
        assert_eq!(result["states"]["36"]["accepted"], 5);
        assert_eq!(result["states"]["36"]["pages"], 3);
        let bytes = fs::read(dir.join("checkpoint-v2.json")).unwrap();
        execute(&c, &dir, false, &mut f).unwrap();
        assert_eq!(f.calls, 3);
        assert_eq!(bytes, fs::read(dir.join("checkpoint-v2.json")).unwrap());
    }
    #[test]
    fn partial_state_advances_on_next_invocation_without_double_counts() {
        let dir = scratch();
        let c = config(1);
        let mut f = fixture();
        for (expected, status) in [(2, "partial"), (4, "partial"), (5, "completed")] {
            let result = execute(&c, &dir, false, &mut f).unwrap();
            assert_eq!(result["states"]["36"]["accepted"], expected);
            assert_eq!(result["states"]["36"]["status"], status);
        }
        let (_, index) = load(&c, &dir).unwrap();
        assert!(index["36"].ids.contains("USGS-000000000000001"));
    }
    #[test]
    fn interruption_retains_partition_and_replays_running_cursor() {
        let dir = scratch();
        let c = config(10);
        let mut f = fixture();
        f.panic_at = Some(1);
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| execute(
                &c, &dir, false, &mut f
            )))
            .is_err()
        );
        let (state, _) = load(&c, &dir).unwrap();
        assert_eq!(state["states"]["36"]["accepted"], 2);
        assert_eq!(state["states"]["36"]["status"], "running");
        let result = execute(&c, &dir, false, &mut f).unwrap();
        assert_eq!(result["states"]["36"]["accepted"], 5);
        assert_eq!(
            result["states"]["36"]["partitions"]
                .as_array()
                .unwrap()
                .len(),
            3
        );
        assert_eq!(
            result["chargedPages"], 4,
            "unknown interrupted reservation remains charged"
        );
    }
    #[test]
    fn orphan_valid_artifacts_reconcile_once_after_commit_gap() {
        let dir = scratch();
        let c = config(1);
        let mut f = fixture();
        execute(&c, &dir, false, &mut f).unwrap();
        let raw = &f.parts[1];
        let bundle = gev_us_groundwater::inventory_bundle(raw, "36").unwrap();
        immutable(&dir, "36-capture", raw).unwrap();
        immutable(&dir, "36-candidate", &encode(&bundle).unwrap()).unwrap();
        fs::write(dir.join("artifact.interrupted.tmp"), b"incomplete").unwrap();
        let result = execute(&c, &dir, false, &mut f).unwrap();
        assert_eq!(result["states"]["36"]["accepted"], 4);
        assert_eq!(
            result["states"]["36"]["partitions"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }
    #[test]
    fn duplicate_page_or_native_id_refuses_new_partition_and_preserves_prior() {
        for bad in [
            capture(Some("A"), Some("B"), &[1, 2]),
            capture(Some("A"), Some("B"), &[2, 3]),
        ] {
            let dir = scratch();
            let c = config(10);
            let mut f = fixture();
            f.parts[1] = bad;
            let result = execute(&c, &dir, false, &mut f).unwrap();
            let r = &result["states"]["36"];
            assert_eq!(r["status"], "failed");
            assert_eq!(r["accepted"], 2);
            assert_eq!(r["partitions"].as_array().unwrap().len(), 1);
            load(&c, &dir).unwrap();
        }
    }
    #[test]
    fn cursor_loops_and_wrong_scope_do_not_advance() {
        for bad in [
            capture(Some("A"), Some("%41"), &[3, 4]),
            capture(Some("wrong"), None, &[3, 4]),
        ] {
            let dir = scratch();
            let c = config(10);
            let mut f = fixture();
            f.parts[1] = bad;
            let result = execute(&c, &dir, false, &mut f).unwrap();
            assert_eq!(result["states"]["36"]["status"], "failed");
            assert_eq!(result["states"]["36"]["accepted"], 2);
        }
    }
    #[test]
    fn resigned_stale_status_cursor_counts_or_ledger_are_rejected() {
        for field in ["status", "nextPage", "accepted", "chargedPages"] {
            let dir = scratch();
            let c = config(1);
            let mut f = fixture();
            execute(&c, &dir, false, &mut f).unwrap();
            let (mut state, _) = load(&c, &dir).unwrap();
            match field {
                "status" => state["states"]["36"][field] = json!("completed"),
                "nextPage" => {
                    state["states"]["36"][field] = json!(first_url("36", 2) + "&cursor=skip")
                }
                "accepted" => state["states"]["36"][field] = json!(99),
                _ => state[field] = json!(0),
            }
            persist(&dir.join("checkpoint-v2.json"), &mut state).unwrap();
            assert!(execute(&c, &dir, false, &mut f).is_err());
            assert_eq!(f.calls, 1);
        }
    }
    #[test]
    fn job_and_run_byte_limits_stop_before_requests_and_resume_remains_partial() {
        let dir = scratch();
        let mut c = config(10);
        c["budgets"]["runBytes"] = json!(PAGE_CHARGE);
        c["budgets"]["jobBytes"] = json!(2 * PAGE_CHARGE);
        let mut f = fixture();
        for expected in [2, 4, 4] {
            let result = execute(&c, &dir, false, &mut f).unwrap();
            assert_eq!(result["states"]["36"]["accepted"], expected);
            assert_eq!(result["states"]["36"]["status"], "partial");
        }
        assert_eq!(f.calls, 2);
    }
    #[test]
    fn malformed_terminal_metadata_does_not_claim_completed() {
        for key in ["links", "numberReturned"] {
            let dir = scratch();
            let c = config(1);
            let mut f = fixture();
            let mut raw: Value = serde_json::from_slice(&f.parts[0]).unwrap();
            raw["acquisitions"][0]["response"]
                .as_object_mut()
                .unwrap()
                .remove(key);
            f.parts[0] = encode(&raw).unwrap();
            let result = execute(&c, &dir, false, &mut f).unwrap();
            assert_eq!(result["states"]["36"]["status"], "failed");
            assert_eq!(result["states"]["36"]["accepted"], 0);
        }
    }
    #[test]
    fn cursor_page_query_self_is_admitted_but_wrong_scope_or_cursor_is_not() {
        for (self_url, valid) in [
            (first_url("36", 2), true),
            (first_url("55", 2), false),
            (format!("{}&cursor=other", first_url("36", 2)), false),
        ] {
            let dir = scratch();
            let c = config(10);
            let mut f = fixture();
            let mut raw: Value = serde_json::from_slice(&f.parts[1]).unwrap();
            raw["acquisitions"][0]["response"]["links"][0]["href"] = json!(self_url);
            f.parts[1] = encode(&raw).unwrap();
            let result = execute(&c, &dir, false, &mut f).unwrap();
            assert_eq!(
                result["states"]["36"]["accepted"],
                if valid { 5 } else { 2 }
            );
            assert_eq!(
                result["states"]["36"]["status"],
                if valid { "completed" } else { "failed" }
            );
        }
    }
    #[test]
    fn failed_state_does_not_abort_other_state_collection() {
        let dir = scratch();
        let mut c = config(10);
        c["states"] = json!([{"state":"55"},{"state":"36"}]);
        let mut f = fixture();
        let result = execute(&c, &dir, false, &mut f).unwrap();
        assert_eq!(result["states"]["55"]["status"], "failed");
        assert_eq!(result["states"]["36"]["status"], "completed");
    }
    #[test]
    fn corrupted_checkpoint_and_partition_fail_before_acquisition() {
        let dir = scratch();
        let c = config(1);
        let mut f = fixture();
        let result = execute(&c, &dir, false, &mut f).unwrap();
        let candidate = result["states"]["36"]["partitions"][0]["candidate"]
            .as_str()
            .unwrap();
        fs::write(dir.join(candidate), b"corrupt test artifact").unwrap();
        assert!(execute(&c, &dir, false, &mut f).is_err());
        assert_eq!(f.calls, 1);
        let dir = scratch();
        fs::write(dir.join("checkpoint-v2.json"), b"{}").unwrap();
        assert!(execute(&c, &dir, false, &mut f).is_err());
        assert_eq!(f.calls, 1);
    }
    #[test]
    fn rate_limit_stops_queue_and_refuses_automatic_retry() {
        struct Limited {
            calls: usize,
        }
        impl InventoryAdapter for Limited {
            fn capture(
                &mut self,
                _: &Value,
                _: &Value,
                _: &Path,
                _: u64,
            ) -> Result<Vec<u8>, String> {
                self.calls += 1;
                Err("HTTP Error 429: Too Many Requests".into())
            }
        }
        let dir = scratch();
        let mut c = config(10);
        c["states"] = json!([{"state":"36"},{"state":"55"}]);
        let mut f = Limited { calls: 0 };
        let result = execute(&c, &dir, false, &mut f).unwrap();
        assert_eq!(result["networkBlocked"], true);
        assert_eq!(result["states"]["55"]["status"], "pending");
        assert!(execute(&c, &dir, false, &mut f).is_err());
        assert_eq!(f.calls, 1);
    }
    #[test]
    fn thousand_row_page_and_short_final_page_remain_deterministic() {
        let dir = scratch();
        let mut c = config(10);
        c["budgets"]["rowsPerPage"] = json!(1000);
        c["budgets"]["bytesPerState"] = json!(8 * MAX_BYTES);
        c["budgets"]["runBytes"] = json!(64 * MAX_BYTES);
        c["budgets"]["jobBytes"] = json!(64 * MAX_BYTES);
        let mut f = fixture();
        f.parts = vec![
            capture(None, Some("A"), &(1..=1000).collect::<Vec<_>>()),
            capture(Some("A"), None, &(1001..=1007).collect::<Vec<_>>()),
        ]
        .into_iter()
        .map(|raw| {
            String::from_utf8(raw)
                .unwrap()
                .replace("limit=2", "limit=1000")
                .into_bytes()
        })
        .collect();
        let result = execute(&c, &dir, false, &mut f).unwrap();
        assert_eq!(result["states"]["36"]["accepted"], 1007);
        assert_eq!(result["states"]["36"]["status"], "completed");
        assert_eq!(result["chargedBytes"], 8 * MAX_BYTES);
        let before = fs::read(dir.join("checkpoint-v2.json")).unwrap();
        execute(&c, &dir, false, &mut f).unwrap();
        assert_eq!(before, fs::read(dir.join("checkpoint-v2.json")).unwrap());
        assert_eq!(f.calls, 2);
        c["budgets"]["rowsPerPage"] = json!(1001);
        assert!(validate_config(&c).is_err());
    }
    struct MultiState {
        calls: Vec<String>,
    }
    impl InventoryAdapter for MultiState {
        fn capture(
            &mut self,
            item: &Value,
            _: &Value,
            _: &Path,
            _: u64,
        ) -> Result<Vec<u8>, String> {
            let code = item["state"].as_str().unwrap();
            self.calls.push(code.to_owned());
            let n = item["_partitionIndex"].as_u64().unwrap();
            let ids = [
                code.parse::<u64>().unwrap() * 1000 + n * 2 + 1,
                code.parse::<u64>().unwrap() * 1000 + n * 2 + 2,
            ];
            let raw = capture(
                if n == 0 { None } else { Some("A") },
                if n == 0 { Some("A") } else { None },
                &ids,
            );
            Ok(String::from_utf8(raw)
                .unwrap()
                .replace("state_code=36", &format!("state_code={code}"))
                .replace(
                    "\"state_code\":\"36\"",
                    &format!("\"state_code\":\"{code}\""),
                )
                .into_bytes())
        }
    }
    #[test]
    fn fair_rounds_and_short_run_resume_rotate_all_fifty_one_jurisdictions() {
        let codes = [
            "01", "02", "04", "05", "06", "08", "09", "10", "11", "12", "13", "15", "16", "17",
            "18", "19", "20", "21", "22", "23", "24", "25", "26", "27", "28", "29", "30", "31",
            "32", "33", "34", "35", "36", "37", "38", "39", "40", "41", "42", "44", "45", "46",
            "47", "48", "49", "50", "51", "53", "54", "55", "56",
        ];
        let dir = scratch();
        let mut c = config(51);
        c["states"] = json!(codes.iter().map(|s| json!({"state":s})).collect::<Vec<_>>());
        c["budgets"]["jobPages"] = json!(102);
        c["budgets"]["runBytes"] = json!(64 * MAX_BYTES);
        c["budgets"]["jobBytes"] = json!(64 * MAX_BYTES);
        let mut f = MultiState { calls: vec![] };
        let result = execute(&c, &dir, false, &mut f).unwrap();
        assert_eq!(f.calls, codes);
        for code in codes {
            assert_eq!(result["states"][code]["status"], "partial");
        }
        let result = execute(&c, &dir, false, &mut f).unwrap();
        for code in codes {
            assert_eq!(result["states"][code]["status"], "completed");
        }
        let dir = scratch();
        c["budgets"]["runPages"] = json!(1);
        let mut f = MultiState { calls: vec![] };
        for _ in 0..3 {
            execute(&c, &dir, false, &mut f).unwrap();
        }
        assert_eq!(f.calls, ["01", "02", "04"]);
    }
    #[test]
    fn disk_allowance_stops_before_adapter_and_preserves_checkpoint() {
        let dir = scratch();
        let mut c = config(1);
        c["budgets"]["jobDiskBytes"] = json!(1024);
        let mut f = fixture();
        let result = execute(&c, &dir, false, &mut f).unwrap();
        assert_eq!(f.calls, 0);
        assert_eq!(result["states"]["36"]["status"], "pending");
        assert_eq!(result["runReport"]["stopReason"], "disk budget");
    }
    #[test]
    fn reported_low_quota_retains_good_partition_and_stops_before_next_state() {
        struct LowQuota {
            inner: Fixture,
        }
        impl InventoryAdapter for LowQuota {
            fn metrics(&self) -> Value {
                json!({"quotaHeaders":[{"X-RateLimit-Limit":"50","X-RateLimit-Remaining":"5"}]})
            }
            fn capture(
                &mut self,
                item: &Value,
                budgets: &Value,
                cache: &Path,
                seconds: u64,
            ) -> Result<Vec<u8>, String> {
                self.inner.capture(item, budgets, cache, seconds)
            }
        }
        let dir = scratch();
        let mut c = config(10);
        c["states"] = json!([{"state":"36"},{"state":"55"}]);
        let mut f = LowQuota { inner: fixture() };
        let result = execute(&c, &dir, false, &mut f).unwrap();
        assert_eq!(result["quotaBlocked"], true);
        assert_eq!(result["states"]["36"]["accepted"], 2);
        assert_eq!(result["states"]["36"]["status"], "partial");
        assert_eq!(result["states"]["55"]["status"], "pending");
        assert!(execute(&c, &dir, false, &mut f).is_err());
        assert_eq!(f.inner.calls, 1);
    }
    #[test]
    fn duplicate_native_ids_across_states_do_not_inflate_national_counts() {
        struct Duplicate {
            inner: MultiState,
        }
        impl InventoryAdapter for Duplicate {
            fn capture(
                &mut self,
                item: &Value,
                budgets: &Value,
                cache: &Path,
                seconds: u64,
            ) -> Result<Vec<u8>, String> {
                let raw = self.inner.capture(item, budgets, cache, seconds)?;
                Ok(String::from_utf8(raw)
                    .unwrap()
                    .replace("000000000022001", "000000000036001")
                    .into_bytes())
            }
        }
        let dir = scratch();
        let mut c = config(2);
        c["states"] = json!([{"state":"36"},{"state":"22"}]);
        let mut f = Duplicate {
            inner: MultiState { calls: vec![] },
        };
        let result = execute(&c, &dir, false, &mut f).unwrap();
        assert_eq!(result["states"]["36"]["accepted"], 2);
        assert_eq!(result["states"]["22"]["accepted"], 0);
        assert_eq!(result["states"]["22"]["status"], "failed");
        assert!(
            result["states"]["22"]["error"]
                .as_str()
                .unwrap()
                .contains("duplicate source ID across state")
        );
    }
    #[test]
    fn amendment_reuses_data_and_preserves_ledger_expiry_and_parent() {
        let dir = scratch();
        let mut cfg = config(1);
        cfg["absoluteExpiry"] = json!(1000000);
        let mut adapter = fixture();
        let prior = execute_with_clock(&cfg, &dir, false, &mut adapter, &|| 1000).unwrap();
        let bytes = fs::read(dir.join("checkpoint-v2.json")).unwrap();
        let files: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| {
                p.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .contains("-capture-")
                    || p.file_name()
                        .unwrap()
                        .to_string_lossy()
                        .contains("-candidate-")
            })
            .map(|p| {
                let b = fs::read(&p).unwrap();
                (p, b)
            })
            .collect();
        let mut new = cfg.clone();
        new["budgets"]["jobPages"] = json!(30);
        let mut smaller_disk_cap = cfg.clone();
        smaller_disk_cap["budgets"]["jobDiskBytes"] = json!(64 * 1024 * 1024);
        assert!(amend(&cfg, &dir, &smaller_disk_cap, 2020, 2000).is_err());
        new["budgets"]["jobDiskBytes"] = json!(2 * 1024 * 1024 * 1024u64);
        new["absoluteExpiry"] = json!(2020);
        let mut mismatched_expiry = new.clone();
        mismatched_expiry["absoluteExpiry"] = json!(2021);
        assert!(amend(&cfg, &dir, &mismatched_expiry, 2020, 2000).is_err());
        let amended = amend(&cfg, &dir, &new, 2020, 2000).unwrap();
        assert_eq!(amended["chargedPages"], prior["chargedPages"]);
        assert_eq!(amended["chargedBytes"], prior["chargedBytes"]);
        assert_eq!(amended["states"], prior["states"]);
        assert_eq!(amended["expiresAtUnixSeconds"], 2020);
        assert_eq!(new["budgets"]["jobDiskBytes"], 2 * 1024 * 1024 * 1024u64);
        assert_eq!(
            amended["continuationParent"]["checkpointSha256"],
            hash(&bytes)
        );
        for (p, b) in files {
            assert_eq!(fs::read(p).unwrap(), b);
        }
        load(&new, &dir).unwrap();
        assert!(amend(&cfg, &dir, &new, 2020, 2000).is_err());
        assert!(execute_with_clock(&new, &dir, false, &mut adapter, &|| 2020).is_err());
    }
    #[test]
    fn amendment_lock_and_interruption_leave_old_checkpoint_usable() {
        let dir = scratch();
        let cfg = config(1);
        let mut adapter = fixture();
        execute_with_clock(&cfg, &dir, false, &mut adapter, &|| 1000).unwrap();
        let original = fs::read(dir.join("checkpoint-v2.json")).unwrap();
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .open(dir.join("runner.lock"))
            .unwrap();
        lock.try_lock().unwrap();
        assert!(
            amend(&cfg, &dir, &cfg, 2020, 2000)
                .unwrap_err()
                .contains("active")
        );
        assert!(!dir.join("amendments").exists());
        drop(lock);
        assert!(
            migration::amend_inner(&cfg, &dir, &cfg, 2020, 2000, true)
                .unwrap_err()
                .contains("interruption")
        );
        assert_eq!(fs::read(dir.join("checkpoint-v2.json")).unwrap(), original);
        load(&cfg, &dir).unwrap();
        amend(&cfg, &dir, &cfg, 2020, 2000).unwrap();
        load(&cfg, &dir).unwrap();
    }
    #[test]
    fn integrated_recovery_refused_before_destination_copy() {
        let dir = scratch();
        let cfg = config(1);
        let mut adapter = fixture();
        execute_with_clock(&cfg, &dir, false, &mut adapter, &|| 1000).unwrap();
        let recovery = scratch();
        fs::create_dir_all(&recovery).unwrap();
        fs::write(
            recovery.join("36-capture.json"),
            capture(None, Some("A"), &[1, 2]),
        )
        .unwrap();
        let target = dir.with_extension("duplicate-recovery");
        assert!(
            migrate(&cfg, &dir, &cfg, &target, 2020, Some(&recovery), 2000)
                .unwrap_err()
                .contains("already integrated")
        );
        assert!(!target.exists());
    }
}
