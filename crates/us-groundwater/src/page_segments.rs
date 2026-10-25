//! Offline large response admission; existing inventory/display limits stay intact.
use super::*;
use std::collections::BTreeSet;
const BODY_CAP: usize = 32 * 1024 * 1024;
const ARTIFACT_CAP: u64 = 4 * 1024 * 1024 * 1024;
const PREFIX: &str =
    "https://api.waterdata.usgs.gov/ogcapi/v1/collections/monitoring-locations/items?";
pub(super) const APPROVED: &str = "id,agency_code,country_code,state_code,county_code,site_type_code,monitoring_location_number,monitoring_location_name,construction_date,well_constructed_depth,hole_constructed_depth,depth_source_code,altitude,altitude_accuracy,altitude_method_code,vertical_datum,vertical_datum_name,horizontal_positional_accuracy_code,horizontal_positional_accuracy,horizontal_position_method_code,horizontal_position_method_name,original_horizontal_datum,original_horizontal_datum_name,aquifer_code,national_aquifer_code,aquifer_type_code,revision_note,revision_created,revision_modified";

pub(super) fn query(url: &str, code: &str, projection: &str) -> Result<String, String> {
    let parts = url
        .strip_prefix(PREFIX)
        .ok_or("fixed source origin/path required")?;
    let mut other = Vec::new();
    let mut selected = None;
    for part in parts.split('&') {
        let (k, v) = part.split_once('=').ok_or("malformed query")?;
        if partitions::decode(k)? == "properties" {
            if selected.replace(partitions::decode(v)?).is_some() {
                return Err("duplicate projection".into());
            }
        } else {
            other.push(part);
        }
    }
    if selected.as_deref() != Some(projection) {
        return Err("source projection changed".into());
    }
    // Reuse existing origin, filters, control rejection and opaque cursor checks.
    if url.len() > 2048
        || url.bytes().any(|b| !(33..=126).contains(&b))
        || url.contains(['#', '\\'])
    {
        return Err("invalid bounded source URL".into());
    }
    partitions::url_key(&format!("{PREFIX}{}", other.join("&")), code, 10000)
}
fn projection(value: &str) -> Result<(), String> {
    let allowed: BTreeSet<_> = APPROVED.split(',').collect();
    let selected: BTreeSet<_> = value.split(',').collect();
    if selected.len() != value.split(',').count()
        || !selected.is_subset(&allowed)
        || ![
            "id",
            "agency_code",
            "country_code",
            "state_code",
            "site_type_code",
            "monitoring_location_number",
            "monitoring_location_name",
            "well_constructed_depth",
            "horizontal_positional_accuracy",
            "original_horizontal_datum",
        ]
        .iter()
        .all(|v| selected.contains(v))
    {
        return Err("unapproved or insufficient well metadata projection".into());
    }
    Ok(())
}
fn disk_used(dir: &Path) -> Result<u64, String> {
    let mut total = 0;
    for entry in fs::read_dir(dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if !entry.file_type().map_err(|e| e.to_string())?.is_file() {
            return Err("segment directory must contain ordinary files only".into());
        }
        total += entry.metadata().map_err(|e| e.to_string())?.len();
    }
    Ok(total)
}
pub(super) fn ingest(repo: &Path, config: &Value, dir: &Path) -> Result<Value, String> {
    ingest_inner(repo, config, dir, None)
}
fn ingest_inner(
    repo: &Path,
    config: &Value,
    dir: &Path,
    stop_after: Option<usize>,
) -> Result<Value, String> {
    if config["schemaVersion"] != 1 || config["kind"] != "usgs-projected-page-reference" {
        return Err("unsupported page reference".into());
    }
    let state = config["state"].as_str().ok_or("state required")?;
    if state.len() != 2
        || !state.bytes().all(|b| b.is_ascii_digit())
        || config["retrievedAt"]
            .as_str()
            .and_then(gev_us_groundwater::time_precision)
            != Some("instant")
    {
        return Err("bounded state and zoned retrieval timestamp required".into());
    }
    let selected = config["projection"].as_str().ok_or("projection required")?;
    projection(selected)?;
    let url = config["url"].as_str().ok_or("source URL required")?;
    let request_key = query(url, state, selected)?;
    let relative = Path::new(config["rawFile"].as_str().ok_or("raw reference required")?);
    if relative
        .components()
        .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return Err("raw reference must be repository relative".into());
    }
    let repo = repo.canonicalize().map_err(|e| e.to_string())?;
    let raw_path = repo
        .join(relative)
        .canonicalize()
        .map_err(|e| e.to_string())?;
    if !raw_path.starts_with(&repo) {
        return Err("raw reference outside repository".into());
    }
    let raw = read_capped(&raw_path, BODY_CAP)?;
    if config["rawSha256"] != hash(&raw) {
        return Err("raw hash mismatch".into());
    }
    let body: Value = serde_json::from_slice(&raw).map_err(|e| e.to_string())?;
    let rows = body["features"]
        .as_array()
        .ok_or("source features required")?;
    if body["type"] != "FeatureCollection"
        || rows.len() > 10000
        || body["numberReturned"].as_u64() != Some(rows.len() as u64)
    {
        return Err("large page type/count limit".into());
    }
    let mut ids = BTreeSet::new();
    let selected_fields: BTreeSet<_> = selected.split(',').collect();
    for row in rows {
        let id = row["id"].as_str().ok_or("source identity required")?;
        if !ids.insert(id) {
            return Err("duplicate source ID across segments".into());
        }
        if row["properties"]
            .as_object()
            .ok_or("source properties required")?
            .keys()
            .any(|key| !selected_fields.contains(key.as_str()))
        {
            return Err("response contains unrequested properties".into());
        }
    }
    let links = body["links"].as_array().ok_or("source links required")?;
    let selves: Vec<_> = links.iter().filter(|l| l["rel"] == "self").collect();
    if selves.len() != 1 {
        return Err("source self link required".into());
    }
    let self_key = query(
        selves[0]["href"].as_str().ok_or("self URL required")?,
        state,
        selected,
    )?;
    if self_key != request_key && self_key != "first" {
        return Err("self cursor mismatch".into());
    }
    let nexts: Vec<_> = links.iter().filter(|l| l["rel"] == "next").collect();
    if nexts.len() > 1 {
        return Err("ambiguous source next".into());
    }
    let next = nexts
        .first()
        .map(|l| l["href"].as_str().ok_or("next URL required"))
        .transpose()?;
    if let Some(next) = next {
        let key = query(next, state, selected)?;
        if key == "first" || key == request_key || rows.is_empty() {
            return Err("source pagination loop".into());
        }
    }
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join("segments.lock"))
        .map_err(|e| e.to_string())?;
    lock.try_lock()
        .map_err(|_| "page admission already active")?;
    let path = dir.join("segments.json");
    let fingerprint = hash(&encode(config)?);
    let mut progress = if path.exists() {
        let envelope: Value =
            serde_json::from_slice(&read_capped(&path, MAX_BYTES)?).map_err(|e| e.to_string())?;
        let state = &envelope["payload"];
        if envelope["schemaVersion"] != 1
            || envelope["sha256"] != hash(&encode(state)?)
            || state["configSha256"] != fingerprint
        {
            return Err("segment progress integrity/config mismatch".into());
        }
        state.clone()
    } else {
        json!({"kind":"usgs-page-segment-manifest","schemaVersion":1,"configSha256":fingerprint,
        "source":config,"inputRows":rows.len(),"segments":[],"status":"pending",
        "committedNext":null,"activation":"candidate-only","generation":0})
    };
    let old_parts = progress["segments"]
        .as_array()
        .ok_or("segment progress required")?
        .clone();
    let segment_count = rows.len().div_ceil(1000);
    if old_parts.len() > segment_count {
        return Err("excess segment progress".into());
    }
    let mut accepted = 0;
    let mut quarantined = 0;
    for (index, chunk) in rows.chunks(1000).enumerate() {
        let mut response: Value = Value::Object(
            body.as_object()
                .unwrap()
                .iter()
                .filter(|(key, _)| key.as_str() != "features")
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        );
        response["features"] = json!(chunk);
        response["numberReturned"] = json!(chunk.len());
        let capture = encode(&json!({"schemaVersion":1,"acquisitions":[{
            "collection":"monitoring-locations","url":url,"retrievedAt":config["retrievedAt"],
            "response":response,"derivation":{"rawResponseSha256":config["rawSha256"],
            "rowOffset":index*1000,"originalReturned":rows.len()}}]}))?;
        let bundle = gev_us_groundwater::inventory_bundle(&capture, state)?;
        let accounting = &bundle["layerPack"]["coverage"]["accounting"];
        accepted += accounting["acceptedWells"]
            .as_u64()
            .ok_or("accepted count required")?;
        quarantined += accounting["quarantined"]
            .as_u64()
            .ok_or("quarantine count required")?;
        let encoded = encode(&bundle)?;
        let capture_name = format!("segment-{index}-capture-{}.json", hash(&capture));
        let candidate_name = format!("segment-{index}-candidate-{}.json", hash(&encoded));
        let part = json!({"index":index,"rowOffset":index*1000,"inputRows":chunk.len(),
            "accepted":accounting["acceptedWells"],"quarantined":accounting["quarantined"],
            "capture":capture_name,"captureSha256":hash(&capture),
            "candidate":candidate_name,"candidateSha256":hash(&encoded)});
        if let Some(old) = old_parts.get(index) {
            if old != &part
                || read_capped(&dir.join(&capture_name), BODY_CAP)? != capture
                || read_capped(&dir.join(&candidate_name), 16 * MAX_BYTES)? != encoded
            {
                return Err("completed segment integrity mismatch".into());
            }
        } else {
            if disk_used(dir)? + capture.len() as u64 + encoded.len() as u64 + MAX_BYTES as u64
                > ARTIFACT_CAP
            {
                return Err("segment artifact disk budget".into());
            }
            immutable(dir, &format!("segment-{index}-capture"), &capture)?;
            immutable(dir, &format!("segment-{index}-candidate"), &encoded)?;
            progress["segments"].as_array_mut().unwrap().push(part);
            save(&path, &mut progress)?;
        }
        if stop_after == Some(index + 1) {
            return Err("simulated interruption after durable segment".into());
        }
    }
    if accepted + quarantined != rows.len() as u64 {
        return Err("page accounting mismatch".into());
    }
    if hash(&read_capped(&raw_path, BODY_CAP)?) != config["rawSha256"] {
        return Err("raw changed during admission".into());
    }
    if progress["status"] == "committed" {
        if progress["committedNext"] != json!(next)
            || progress["accepted"] != accepted
            || progress["quarantined"] != quarantined
        {
            return Err("committed page accounting/cursor mismatch".into());
        }
        return Ok(progress);
    }
    if !progress["committedNext"].is_null() {
        return Err("uncommitted page advanced cursor".into());
    }
    progress["accepted"] = json!(accepted);
    progress["quarantined"] = json!(quarantined);
    progress["status"] = json!("committed");
    progress["committedNext"] = json!(next);
    save(&path, &mut progress)?;
    Ok(progress)
}

#[cfg(test)]
mod tests {
    use super::*;
    static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    fn setup(count: usize, null_last: bool, invalid_last: bool) -> (PathBuf, Value, PathBuf) {
        let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../.temp-gev")
            .join(format!(
                "segments-test-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
        let repo = repo.join(
            SEQUENCE
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                .to_string(),
        );
        fs::create_dir_all(&repo).unwrap();
        let url = format!(
            "{PREFIX}f=json&limit=10000&agency_code=USGS&country_code=US&site_type_code=GW&state_code=36&properties={APPROVED}"
        );
        let rows: Vec<Value> = (0..count)
            .map(|n| {
                let id = format!("USGS-{:015}", 100000000000000u64 + n as u64);
                let geometry = if n + 1 == count && null_last {
                    Value::Null
                } else if n + 1 == count && invalid_last {
                    json!({"type":"Point","coordinates":[500,40]})
                } else {
                    json!({"type":"Point","coordinates":[-79.0,42.0]})
                };
                json!({"type":"Feature","id":id,"geometry":geometry,"properties":{
                "id":id,"agency_code":"USGS","country_code":"US","state_code":"36",
                "site_type_code":"GW","monitoring_location_name":"synthetic test only"}})
            })
            .collect();
        let raw = encode(&json!({"type":"FeatureCollection","numberReturned":count,"features":rows,
            "links":[{"rel":"self","href":url},{"rel":"next","href":format!("{url}&cursor=opaque-next")}]})).unwrap();
        fs::write(repo.join("raw.json"), &raw).unwrap();
        let config = json!({"schemaVersion":1,"kind":"usgs-projected-page-reference","state":"36",
            "projection":APPROVED,"url":url,"retrievedAt":"2026-10-25T00:00:00Z",
            "rawFile":"raw.json","rawSha256":hash(&raw)});
        let dir = repo.join("segments");
        (repo, config, dir)
    }
    #[test]
    fn multi_segment_short_tail_quarantine_and_unchanged_replay() {
        let (repo, config, dir) = setup(2001, true, false);
        let result = ingest(&repo, &config, &dir).unwrap();
        assert_eq!(result["segments"].as_array().unwrap().len(), 3);
        assert_eq!(result["segments"][2]["inputRows"], 1);
        assert_eq!(result["accepted"], 2000);
        assert_eq!(result["quarantined"], 1);
        assert!(
            result["committedNext"]
                .as_str()
                .unwrap()
                .ends_with("cursor=opaque-next")
        );
        let before = fs::read(dir.join("segments.json")).unwrap();
        assert_eq!(result, ingest(&repo, &config, &dir).unwrap());
        assert_eq!(before, fs::read(dir.join("segments.json")).unwrap());
    }
    #[test]
    fn crash_resume_reuses_durable_segment_without_duplicate_counts() {
        let (repo, config, dir) = setup(1001, false, false);
        assert!(ingest_inner(&repo, &config, &dir, Some(1)).is_err());
        let envelope: Value =
            serde_json::from_slice(&fs::read(dir.join("segments.json")).unwrap()).unwrap();
        assert_eq!(envelope["payload"]["status"], "pending");
        assert!(envelope["payload"]["committedNext"].is_null());
        let name = envelope["payload"]["segments"][0]["candidate"]
            .as_str()
            .unwrap();
        let before = fs::metadata(dir.join(name)).unwrap().modified().unwrap();
        let result = ingest(&repo, &config, &dir).unwrap();
        assert_eq!(result["accepted"], 1001);
        assert_eq!(
            fs::metadata(dir.join(name)).unwrap().modified().unwrap(),
            before
        );
    }
    #[test]
    fn invalid_nonnull_geometry_preserves_partial_progress_and_cursor() {
        let (repo, config, dir) = setup(1001, false, true);
        let failure = ingest(&repo, &config, &dir).unwrap_err();
        assert!(
            failure.contains("invalid authoritative coordinates"),
            "{failure}"
        );
        let before = fs::read(dir.join("segments.json")).unwrap();
        let envelope: Value = serde_json::from_slice(&before).unwrap();
        assert!(envelope["payload"]["committedNext"].is_null());
        assert_eq!(envelope["payload"]["segments"].as_array().unwrap().len(), 1);
        assert!(ingest(&repo, &config, &dir).is_err());
        assert_eq!(before, fs::read(dir.join("segments.json")).unwrap());
    }
    #[test]
    fn tampered_raw_or_segment_and_duplicate_ids_fail_closed() {
        let (repo, config, dir) = setup(1001, false, false);
        ingest_inner(&repo, &config, &dir, Some(1)).unwrap_err();
        let env: Value =
            serde_json::from_slice(&fs::read(dir.join("segments.json")).unwrap()).unwrap();
        fs::write(
            dir.join(env["payload"]["segments"][0]["candidate"].as_str().unwrap()),
            b"tampered",
        )
        .unwrap();
        assert!(
            ingest(&repo, &config, &dir)
                .unwrap_err()
                .contains("integrity")
        );
        let mut body: Value =
            serde_json::from_slice(&fs::read(repo.join("raw.json")).unwrap()).unwrap();
        body["features"][1000] = body["features"][0].clone();
        let bytes = encode(&body).unwrap();
        fs::write(repo.join("raw.json"), &bytes).unwrap();
        assert!(
            ingest(&repo, &config, &dir)
                .unwrap_err()
                .contains("raw hash")
        );
        let mut changed = config.clone();
        changed["rawSha256"] = json!(hash(&bytes));
        assert!(
            ingest(&repo, &changed, &repo.join("duplicate-output"))
                .unwrap_err()
                .contains("duplicate source ID")
        );
    }
    #[test]
    fn scope_projection_and_key_urls_are_strict() {
        let (repo, config, dir) = setup(1, false, false);
        let url = config["url"].as_str().unwrap();
        for bad in [
            url.replace("state_code=36", "state_code=22"),
            url.replace("limit=10000", "limit=50000"),
            url.replace("api.waterdata.usgs.gov", "example.org"),
            format!("{url}&api_key=not-allowed"),
        ] {
            assert!(query(&bad, "36", APPROVED).is_err());
        }
        assert!(projection("id,owner_name").is_err());
        ingest(&repo, &config, &dir).unwrap();
    }
}
