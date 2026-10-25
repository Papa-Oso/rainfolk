//! Segmented-page adapter for the existing v2 queue, not another scheduler.
use super::*;
use crate::page_segments;
fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}
pub(super) fn enabled(config: &Value) -> bool {
    config["projection"].is_string()
}
pub(super) fn version(record: &Value) -> Result<String, String> {
    let fields: serde_json::Map<String, Value> = page_segments::APPROVED
        .split(',')
        .map(|key| (key.into(), record["metadata"][key].clone()))
        .collect();
    Ok(hash(&encode(
        &json!({"id":record["id"],"longitude":record["longitude"],
        "latitude":record["latitude"],"metadata":fields}),
    )?))
}
pub(super) fn baseline(config: &Value) -> Result<BTreeMap<String, String>, String> {
    let mut ids = BTreeMap::new();
    if !enabled(config) || config["mode"] != "live" {
        return Ok(ids);
    }
    let root = repo().canonicalize().map_err(|e| e.to_string())?;
    let rel = Path::new(
        config["baselineCheckpoint"]
            .as_str()
            .ok_or("baseline checkpoint required")?,
    );
    if rel
        .components()
        .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return Err("repository-relative baseline required".into());
    }
    let path = root.join(rel).canonicalize().map_err(|e| e.to_string())?;
    if !path.starts_with(&root) {
        return Err("baseline outside repository".into());
    }
    let bytes = read_capped(&path, CHECKPOINT_CAP)?;
    if config["baselineSha256"] != hash(&bytes) {
        return Err("baseline hash changed".into());
    }
    let envelope: Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    let old = &envelope["payload"];
    if envelope["sha256"] != hash(&encode(old)?) {
        return Err("baseline envelope integrity".into());
    }
    for item in config["states"].as_array().ok_or("state queue required")? {
        let code = item["state"].as_str().ok_or("state required")?;
        if old["states"][code]["status"] != "partial" {
            return Err("only partial baseline jurisdictions admitted".into());
        }
    }
    let completed: BTreeSet<String> = old["states"]
        .as_object()
        .ok_or("baseline states required")?
        .iter()
        .filter(|(_, s)| s["status"] == "completed")
        .map(|(code, _)| code.clone())
        .collect();
    let skipped: BTreeSet<String> = config["skippedExhausted"]
        .as_array()
        .ok_or("explicit skipped table required")?
        .iter()
        .map(|s| s["state"].as_str().unwrap_or("").to_owned())
        .collect();
    if completed != skipped {
        return Err("skipped jurisdiction table differs from exhausted baseline".into());
    }
    for state in old["states"].as_object().unwrap().values() {
        for part in state["partitions"]
            .as_array()
            .ok_or("baseline partitions required")?
        {
            let name = part["candidate"]
                .as_str()
                .ok_or("baseline candidate required")?;
            if !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
            {
                return Err("invalid baseline name".into());
            }
            let bytes = read_capped(&path.parent().unwrap().join(name), 16 * MAX_BYTES)?;
            if part["candidateSha256"] != hash(&bytes) {
                return Err("baseline candidate hash".into());
            }
            let bundle: Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
            for record in bundle["inventory"]["records"]
                .as_array()
                .ok_or("baseline inventory required")?
            {
                let id = record["id"].as_str().ok_or("baseline identity required")?;
                if ids.insert(id.into(), version(record)?).is_some() {
                    return Err("duplicate baseline identity".into());
                }
            }
        }
    }
    if config["baselineAccepted"] != ids.len() {
        return Err("baseline accepted count mismatch".into());
    }
    Ok(ids)
}
pub(super) fn admit(
    config: &Value,
    dir: &Path,
    code: &str,
    capture: &Value,
    baseline: &BTreeMap<String, String>,
) -> Result<(Value, Value), String> {
    let pages = capture["acquisitions"]
        .as_array()
        .ok_or("capture pages required")?;
    if pages.len() != 1 {
        return Err("one projected acquisition per partition".into());
    }
    let page = &pages[0];
    page_segments::query(
        page["url"].as_str().ok_or("page URL required")?,
        code,
        config["projection"].as_str().ok_or("projection required")?,
    )?;
    let root = repo().canonicalize().map_err(|e| e.to_string())?;
    let raw = Path::new(
        page["rawFile"]
            .as_str()
            .ok_or("raw file reference required")?,
    )
    .canonicalize()
    .map_err(|e| e.to_string())?;
    let rel = raw
        .strip_prefix(&root)
        .map_err(|_| "raw outside repository")?;
    let reference = json!({"schemaVersion":1,"kind":"usgs-projected-page-reference","state":code,
        "projection":config["projection"],"url":page["url"],"retrievedAt":page["retrievedAt"],
        "rawFile":rel.to_string_lossy().replace('\\',"/"),"rawSha256":page["rawSha256"]});
    let name = immutable(dir, &format!("{code}-page-reference"), &encode(&reference)?)?;
    let segment_dir = format!(
        "page-{code}-{}",
        reference["rawSha256"].as_str().ok_or("raw hash required")?
    );
    let manifest = page_segments::ingest(&root, &reference, &dir.join(&segment_dir))?;
    let mut records = Vec::new();
    let mut quarantine = Vec::new();
    let mut overlap = 0;
    let mut changed = 0;
    for segment in manifest["segments"].as_array().ok_or("segments required")? {
        let bytes = read_capped(
            &dir.join(&segment_dir)
                .join(segment["candidate"].as_str().unwrap()),
            16 * MAX_BYTES,
        )?;
        let bundle: Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        for row in bundle["inventory"]["records"].as_array().unwrap() {
            let id = row["id"].as_str().unwrap();
            let fingerprint = version(row)?;
            if let Some(old) = baseline.get(id) {
                overlap += 1;
                if old != &fingerprint {
                    changed += 1;
                }
            }
            records.push(json!({"id":id,"versionSha256":fingerprint}));
        }
        if let Some(rows) = bundle["layerPack"]["coverage"]["quarantine"].as_array() {
            quarantine.extend(rows.iter().map(|r| json!({"sourceId":r["sourceId"]})));
        }
    }
    let part = json!({"pageReference":name,"pageReferenceSha256":hash(&encode(&reference)?),
        "segmentDirectory":segment_dir,"segmentManifestSha256":hash(&read_capped(&dir.join(&segment_dir).join("segments.json"),MAX_BYTES)?),
        "rows":records.len(),"quarantined":quarantine.len(),"rawRows":records.len()+quarantine.len(),
        "overlapAccepted":overlap,"changedOverlap":changed,"newAccepted":records.len()-overlap,
        "pages":1,"nextPage":manifest["committedNext"]});
    Ok((
        json!({"inventory":{"records":records},"layerPack":{"coverage":{"quarantine":quarantine}}}),
        part,
    ))
}
pub(super) fn restore(
    config: &Value,
    dir: &Path,
    code: &str,
    part: &Value,
    baseline: &BTreeMap<String, String>,
) -> Result<(Value, Value), String> {
    let digest = part["pageReferenceSha256"]
        .as_str()
        .ok_or("page reference digest")?;
    if digest.len() != 64
        || !digest.bytes().all(|b| b.is_ascii_hexdigit())
        || part["pageReference"] != format!("{code}-page-reference-{digest}.json")
    {
        return Err("page reference name".into());
    }
    let reference: Value = serde_json::from_slice(&read_capped(
        &dir.join(part["pageReference"].as_str().unwrap()),
        65536,
    )?)
    .map_err(|e| e.to_string())?;
    if hash(&encode(&reference)?) != digest {
        return Err("page reference integrity".into());
    }
    let segment_path = dir.join(format!(
        "page-{code}-{}",
        reference["rawSha256"]
            .as_str()
            .ok_or("raw digest required")?
    ));
    if !segment_path.join("segments.json").exists() {
        return Err("missing committed segment manifest".into());
    }
    let raw_path = repo().join(
        reference["rawFile"]
            .as_str()
            .ok_or("raw reference required")?,
    );
    let root = repo().canonicalize().map_err(|e| e.to_string())?;
    let raw_path = raw_path.canonicalize().map_err(|e| e.to_string())?;
    if !raw_path.starts_with(&root) {
        return Err("raw reference outside repository".into());
    }
    let raw = read_capped(&raw_path, 32 * 1024 * 1024)?;
    let body: Value = serde_json::from_slice(&raw).map_err(|e| e.to_string())?;
    let capture = json!({"schemaVersion":1,"acquisitions":[{"collection":"monitoring-locations",
        "url":reference["url"],"retrievedAt":reference["retrievedAt"],"rawFile":raw_path,
        "rawSha256":reference["rawSha256"],"response":body}]});
    let (summary, regenerated) = admit(config, dir, code, &capture, baseline)?;
    if &regenerated != part {
        return Err("projected partition reconciliation".into());
    }
    Ok((capture, summary))
}

#[cfg(test)]
mod tests {
    use super::*;
    static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    struct Cached {
        capture: Value,
        calls: usize,
        fail: bool,
        remaining: &'static str,
    }
    impl InventoryAdapter for Cached {
        fn metrics(&self) -> Value {
            json!({"networkRequests":0,"cacheHits":1,"rawResponseBytes":100,
            "quotaHeaders":[{"X-RateLimit-Limit":"1000","X-RateLimit-Remaining":self.remaining}]})
        }
        fn capture(&mut self, _: &Value, _: &Value, _: &Path, _: u64) -> Result<Vec<u8>, String> {
            self.calls += 1;
            if self.fail {
                self.fail = false;
                return Err("simulated interruption after capture".into());
            }
            encode(&self.capture)
        }
    }
    fn setup(next: bool) -> (Value, PathBuf, Cached) {
        let root = repo();
        let dir = root.join(".temp-gev").join(format!(
            "projected-queue-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let dir = dir.join(
            SEQUENCE
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                .to_string(),
        );
        fs::create_dir_all(&dir).unwrap();
        let url = format!(
            "{}&properties={}",
            first_url("36", 10000),
            page_segments::APPROVED.replace(',', "%2C")
        );
        let mut links = vec![json!({"rel":"self","href":url})];
        if next {
            links.push(json!({"rel":"next","href":format!("{url}&cursor=next")}));
        }
        let rows:Vec<_>=(0..1001).map(|n|{let id=format!("USGS-{:015}",100000000000000u64+n);
            json!({"id":id,"type":"Feature","geometry":{"type":"Point","coordinates":[-79.0,42.0]},
                "properties":{"id":id,"agency_code":"USGS","country_code":"US","state_code":"36","site_type_code":"GW","monitoring_location_name":"synthetic"}})}).collect();
        let body = json!({"type":"FeatureCollection","numberReturned":rows.len(),"features":rows,"links":links});
        let raw = encode(&body).unwrap();
        let file = dir.join("raw.json");
        fs::write(&file, &raw).unwrap();
        let capture = json!({"schemaVersion":1,"acquisitions":[{"collection":"monitoring-locations","url":url,
            "retrievedAt":"2026-10-25T00:00:00Z","rawFile":file,"rawSha256":hash(&raw),"response":body}]});
        let config = json!({"schemaVersion":2,"adapter":"usgs-monitoring-locations","mode":"fixture",
            "projection":page_segments::APPROVED,"stopOnFailure":true,"states":[{"state":"36"}],
            "budgets":{"pages":1,"rowsPerPage":10000,"bytesPerState":67108864,"stateSeconds":120,"runSeconds":30,
                "rateMillis":5000,"runPages":1,"jobPages":3,"runBytes":33554432,"jobBytes":100663296,"jobDiskBytes":4294967296u64}});
        (
            config,
            dir,
            Cached {
                capture,
                calls: 0,
                fail: false,
                remaining: "900",
            },
        )
    }
    #[test]
    fn queue_commits_segments_and_completed_replay_does_not_refetch() {
        let (config, dir, mut adapter) = setup(false);
        let result = execute_with_clock(&config, &dir, false, &mut adapter, &|| 1000).unwrap();
        assert_eq!(result["states"]["36"]["accepted"], 1001);
        assert_eq!(result["states"]["36"]["status"], "completed");
        let replay = execute_with_clock(&config, &dir, false, &mut adapter, &|| 1001).unwrap();
        assert_eq!(adapter.calls, 1);
        assert_eq!(replay["runReport"]["cacheHits"], 0);
        assert_eq!(replay["runReport"]["coverage"]["streamAccepted"], 1001);
    }
    #[test]
    fn captured_page_and_durable_segments_resume_without_refetch_or_cursor_skip() {
        let (config, dir, mut adapter) = setup(true);
        admit(&config, &dir, "36", &adapter.capture, &BTreeMap::new()).unwrap();
        adapter.fail = true;
        let failed = execute_with_clock(&config, &dir, false, &mut adapter, &|| 1000).unwrap();
        assert!(failed["states"]["36"]["nextPage"].is_null());
        assert_eq!(failed["states"]["36"]["accepted"], 0);
        let result = execute_with_clock(&config, &dir, false, &mut adapter, &|| 1001).unwrap();
        assert_eq!(result["states"]["36"]["accepted"], 1001);
        assert_eq!(result["runReport"]["networkRequests"], 0);
        assert_eq!(result["runReport"]["cacheHits"], 1);
        assert_eq!(result["states"]["36"]["pages"], 1);
        assert!(
            result["states"]["36"]["nextPage"]
                .as_str()
                .unwrap()
                .ends_with("cursor=next")
        );
    }
    #[test]
    fn quota_rate_byte_and_expiry_stops_remain_bounded() {
        let (mut config, dir, mut adapter) = setup(true);
        config["budgets"]["runPages"] = json!(2);
        config["budgets"]["runBytes"] = json!(67108864);
        adapter.remaining = "99";
        let result = execute_with_clock(&config, &dir, false, &mut adapter, &|| 1000).unwrap();
        assert_eq!(adapter.calls, 1);
        assert_eq!(result["quotaBlocked"], true);
        assert!(execute_with_clock(&config, &dir, false, &mut adapter, &|| 1001).is_err());
        let (mut bad, dir, mut adapter) = setup(true);
        bad["budgets"]["rateMillis"] = json!(4999);
        assert!(execute_with_clock(&bad, &dir, false, &mut adapter, &|| 1000).is_err());
        assert_eq!(adapter.calls, 0);
        let (mut limited, dir, mut adapter) = setup(true);
        limited["budgets"]["runBytes"] = json!(33554431);
        let result = execute_with_clock(&limited, &dir, false, &mut adapter, &|| 1000).unwrap();
        assert_eq!(adapter.calls, 0);
        assert_eq!(result["chargedPages"], 0);
        let (mut expired, dir, mut adapter) = setup(true);
        expired["absoluteExpiry"] = json!(999);
        assert!(execute_with_clock(&expired, &dir, false, &mut adapter, &|| 1000).is_err());
        assert_eq!(adapter.calls, 0);
    }
    #[test]
    fn overlap_accounting_preserves_changed_versions_and_prior_provenance() {
        let (config, dir, adapter) = setup(false);
        let mut baseline = BTreeMap::new();
        baseline.insert("USGS-100000000000000".into(), "older-version".into());
        let (_, part) = admit(&config, &dir, "36", &adapter.capture, &baseline).unwrap();
        assert_eq!(part["rows"], 1001);
        assert_eq!(part["overlapAccepted"], 1);
        assert_eq!(part["changedOverlap"], 1);
        assert_eq!(part["newAccepted"], 1000);
        let (_, again) = restore(&config, &dir, "36", &part, &baseline).unwrap();
        assert_eq!(
            again["inventory"]["records"].as_array().unwrap().len(),
            1001
        );
    }
}
