//! Bounded local USGWD CSV snapshots; no acquisition, scheduler or index activation.
use super::*;
use std::io::{BufRead, BufReader};
const RESOURCE: &str = "8b02895f02c14dd1a749bcc5584a5c55";
const INPUT_CAP: u64 = 192 * 1024 * 1024;
const ROW_CAP: usize = 256 * 1024;
const ROWS: usize = 250_000;
const CHECKPOINT_CAP: usize = MAX_BYTES;
const MANIFEST_CAP: usize = MAX_BYTES;
const OUTPUT_CAP: u64 = 2 * 1024 * 1024 * 1024;
const OUTPUT_RESERVE: u64 = (CHECKPOINT_CAP + MANIFEST_CAP) as u64;
const RUN_SECONDS: u64 = 300;

fn deadline(started: Instant) -> Result<(), String> {
    if started.elapsed() >= Duration::from_secs(RUN_SECONDS) {
        return Err("USGWD invocation time cap; checkpoint retained".into());
    }
    Ok(())
}
// Match the shared atomic writer's actual envelope, including its generation.
// Refuse before save so a checkpoint can never exceed the resume reader's cap.
fn envelope_bytes(payload: &Value) -> Result<usize, String> {
    let mut next = payload.clone();
    next["generation"] = json!(next["generation"].as_u64().unwrap_or(0) + 1);
    Ok(encode(&json!({"schemaVersion":1,"sha256":hash(&encode(&next)?),"payload":next}))?.len())
}
fn save_bounded(
    path: &Path,
    payload: &mut Value,
    cap: usize,
    started: Instant,
) -> Result<(), String> {
    deadline(started)?;
    if envelope_bytes(payload)? > cap {
        return Err("USGWD checkpoint/manifest byte cap; previous checkpoint retained".into());
    }
    deadline(started)?;
    save(path, payload)
}
struct DigestReader {
    file: File,
    sha: Sha256,
    bytes: u64,
}
impl Read for DigestReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.file.read(buf)?;
        self.bytes += n as u64;
        if self.bytes > INPUT_CAP {
            return Err(std::io::Error::other("CSV input cap"));
        }
        self.sha.update(&buf[..n]);
        Ok(n)
    }
}
const HEADER: [&str; 29] = [
    "Well ID",
    "Well ID (State)",
    "Longitude",
    "Latitude",
    "County",
    "State",
    "FIPS",
    "Aquifer-Specific",
    "Aquifer-Broad",
    "Subwatershed-HUC12",
    "Subwatershed-Name",
    "Location Verified",
    "Flag County",
    "Flag State",
    "Flag US",
    "Well Depth (Feet)",
    "Screen Depth (Feet)",
    "Length of Screen (Feet)",
    "Well Capacity (GPM)",
    "Lithological Data",
    "Surface Elevation (Feet)",
    "Status",
    "Year Well was Constructed",
    "Year Reported",
    "USGS Water Use Category",
    "Irrigation Subcategory (State)",
    "Irrigation Subcategory (Land Use)",
    "Water Quality (Potable/Non-Potable)",
    "Flag Duplicate",
];

// RFC4180 quoting, escaped quotes and embedded newlines. Structural corruption
// fails the stream: guessing a row boundary could associate data with a wrong ID.
fn csv_row<R: BufRead>(r: &mut R) -> Result<Option<Vec<Vec<u8>>>, String> {
    let (mut fields, mut field, mut state, mut size) = (Vec::new(), Vec::new(), 0, 0);
    loop {
        let bytes = r.fill_buf().map_err(|_| "CSV read failure")?;
        if bytes.is_empty() {
            if state == 1 {
                return Err("unterminated CSV quote".into());
            }
            if size == 0 {
                return Ok(None);
            }
            fields.push(field);
            return Ok(Some(fields));
        }
        let b = bytes[0];
        r.consume(1);
        size += 1;
        if size > ROW_CAP || field.len() > 4096 || fields.len() > HEADER.len() {
            return Err("CSV row/field cap".into());
        }
        if state == 1 {
            if b == b'"' {
                state = 2;
            } else {
                field.push(b);
            }
            continue;
        }
        if state == 2 && b == b'"' {
            field.push(b);
            state = 1;
            continue;
        }
        match b {
            b'"' if state == 0 && field.is_empty() => state = 1,
            b',' => {
                fields.push(std::mem::take(&mut field));
                state = 0;
            }
            b'\n' | b'\r' => {
                if b == b'\r' {
                    if r.fill_buf().map_err(|_| "CSV read failure")?.first() != Some(&b'\n') {
                        return Err("bare CSV carriage return".into());
                    }
                    r.consume(1);
                }
                fields.push(field);
                return Ok(Some(fields));
            }
            _ if state == 2 || b == b'"' => return Err("invalid CSV quoting".into()),
            _ => field.push(b),
        }
    }
}
fn text(fields: Vec<Vec<u8>>) -> Result<Vec<String>, &'static str> {
    fields
        .into_iter()
        .map(|b| String::from_utf8(b).map_err(|_| "invalid-utf8"))
        .collect()
}
fn hex(v: &Value) -> bool {
    v.as_str()
        .is_some_and(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
}
fn missing(s: &str) -> bool {
    s.trim().is_empty() || s.trim().eq_ignore_ascii_case("unknown")
}
fn native(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
}
// Only documented integer spellings, with surrounding whitespace removed.
// Decimal/scientific spellings and unknown codes are not silently coerced.
fn flag(s: &str, max: u8) -> Option<u8> {
    match s.trim() {
        "0" => Some(0),
        "1" => Some(1),
        "2" => Some(2),
        "3" => Some(3),
        _ => None,
    }
    .filter(|n| *n <= max)
}
fn normalized(f: &[String], source: &Value) -> Result<Value, &'static str> {
    if f.len() != HEADER.len() {
        return Err("column-count");
    }
    if !native(&f[0]) {
        return Err("invalid-release-well-id");
    }
    if f[5] != source["state"].as_str().unwrap() {
        return Err("state-file-mismatch");
    }
    if f.iter().any(|s| s.len() > 4096 || s.contains('\0')) {
        return Err("invalid-field");
    }
    let mut values = serde_json::Map::new();
    for (k, v) in HEADER.iter().zip(f) {
        values.insert((*k).into(), json!(v));
    }
    let mut quantities = serde_json::Map::new();
    for (i, unit) in [
        (15, "ft"),
        (16, "ft"),
        (17, "ft"),
        (18, "gal/min"),
        (20, "ft"),
    ] {
        let n = if missing(&f[i]) {
            None
        } else {
            Some(
                f[i].trim()
                    .parse::<f64>()
                    .ok()
                    .filter(|n| n.is_finite() && (i == 20 || *n >= 0.0))
                    .ok_or("invalid-quantity")?,
            )
        };
        quantities.insert(HEADER[i].into(),json!({"value":n.map(|n| if n==0.0 {0.0} else {n}),"unit":unit,"verticalReference":if i==20 {"source-sea-level; datum-unspecified"} else {"source-definition"}}));
    }
    let mut years = serde_json::Map::new();
    for i in [22, 23] {
        let v = if missing(&f[i]) {
            Value::Null
        } else {
            if f[i].len() != 4 || !f[i].bytes().all(|b| b.is_ascii_digit()) || f[i] == "0000" {
                return Err("invalid-year");
            }
            json!({"year":f[i],"precision":"year","timeZone":null})
        };
        years.insert(HEADER[i].into(), v);
    }
    let lon = f[2]
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|n| n.is_finite() && (-180.0..=180.0).contains(n));
    let lat = f[3]
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|n| n.is_finite() && (-90.0..=90.0).contains(n));
    let spatial = lon.is_some() && lat.is_some();
    let flags_valid = [12, 13, 14].iter().all(|i| flag(&f[*i], 2).is_some())
        && flag(&f[28], 3).is_some()
        && matches!(f[11].trim(), "Yes" | "No" | "Unknown");
    let eligible =
        spatial && flags_valid && flag(&f[13], 2) != Some(1) && flag(&f[14], 2) != Some(1);
    Ok(
        json!({"id":format!("USGWD-{}-{}",source["releaseSha256"].as_str().unwrap(),f[0]),
        "source":"USGWD","sourceRecordId":f[0],"stateRecordId":if missing(&f[1]) {Value::Null} else {json!(f[1])},
        "identityScope":"published-release; not stable across releases","sourceSystem":null,
        "recordType":"historical-well-catalog-record","sourceValues":values,"quantities":quantities,"sourceYears":years,
        "sourceCoordinates":{"longitude":f[2],"latitude":f[3],"crs":"EPSG:4269","datum":"NAD83","unit":"decimal-degrees"},
        "longitude":lon,"latitude":lat,"coordinateReference":"EPSG:4269","coordinateAccuracy":"source-quality-flags; no surveyed-wellhead inference",
        "spatial":spatial,"mapEligible":eligible,"mapEligibilityScope":"requires CRS-aware consumer; no EPSG:4326 conversion applied",
        "mapSuppressionReason":if !spatial {json!("missing-or-invalid-coordinates")} else if !flags_valid {json!("invalid-source-quality-flag")} else if !eligible {json!("source-state-or-US-inconsistency")} else {Value::Null},
        "existingIndexEligible":false,"existingIndexLimitation":"NAD83 conversion and candidate contract bridge deferred",
        "fips":f[6],"huc12":f[9],"activeWellStatus":"source-reported; not independently verified","waterSafetyStatus":"not-assessed"}),
    )
}
fn source(path: &Path) -> Result<Value, String> {
    source_with_cap(path, INPUT_CAP)
}
fn source_with_cap(path: &Path, input_cap: u64) -> Result<Value, String> {
    let s = gev_us_groundwater::texas_sdr::decode_capture_json(&read_capped(path, MAX_BYTES)?)?;
    let state = s["state"]
        .as_str()
        .filter(|s| {
            !s.is_empty()
                && s.len() <= 64
                && s.bytes().all(|b| b.is_ascii_alphabetic() || b == b' ')
        })
        .ok_or("invalid state")?;
    let expected = format!(
        "https://www.hydroshare.org/resource/{RESOURCE}/data/contents/USGWD-Tabular/USGWD_{}.csv",
        state.replace(' ', "_")
    );
    let allowed = [
        "schemaVersion",
        "resourceId",
        "releaseSha256",
        "inputSha256",
        "inputBytes",
        "state",
        "sourceUrl",
        "checkedAt",
        "sourcePublishedAt",
        "sourceFileModifiedAt",
        "sourcePublishedTimeZone",
        "license",
        "attribution",
        "schemaSha256",
    ];
    if s.as_object().is_none_or(|o| {
        o.len() != allowed.len() || o.keys().any(|k| !allowed.contains(&k.as_str()))
    }) || s["schemaVersion"] != 1
        || s["resourceId"] != RESOURCE
        || s["sourceUrl"] != expected
        || s["license"] != "CC-BY-4.0"
        || !hex(&s["releaseSha256"])
        || !hex(&s["inputSha256"])
        || !hex(&s["schemaSha256"])
        || s["inputBytes"]
            .as_u64()
            .is_none_or(|n| n == 0 || n > input_cap)
        || s["checkedAt"]
            .as_str()
            .and_then(gev_us_groundwater::time_precision)
            != Some("instant")
        || !s["sourcePublishedTimeZone"].is_null()
        || !s["sourcePublishedAt"].is_null()
        || s["sourceFileModifiedAt"]
            .as_str()
            .is_none_or(|v| v.len() > 64 || v.chars().any(char::is_control))
        || s["attribution"]
            .as_str()
            .is_none_or(|v| v.is_empty() || v.len() > 4096 || v.chars().any(char::is_control))
    {
        return Err("invalid USGWD source receipt".into());
    }
    Ok(s)
}
fn part(
    dir: &Path,
    kind: &str,
    v: &Value,
    used: &mut u64,
    started: Instant,
) -> Result<Value, String> {
    deadline(started)?;
    let bytes = encode(v)?;
    if bytes.len() > 8 * MAX_BYTES || *used + bytes.len() as u64 + OUTPUT_RESERVE > OUTPUT_CAP {
        return Err("USGWD artifact/output cap".into());
    }
    deadline(started)?;
    let name = immutable(dir, kind, &bytes)?;
    *used += bytes.len() as u64;
    Ok(json!({"path":name,"sha256":hash(&bytes),"bytes":bytes.len()}))
}
pub(super) fn run(
    csv: &Path,
    receipt: &Path,
    dir: &Path,
    allowance: usize,
) -> Result<Value, String> {
    if !(1..=100).contains(&allowance) {
        return Err("new partition allowance must be 1..100".into());
    }
    ingest(csv, receipt, dir, allowance)
}
// One process keeps reconciliation state across batches. The maximum number of
// new partitions derives from the existing file-row cap, not an unbounded loop.
pub(super) fn run_full(csv: &Path, receipt: &Path, dir: &Path) -> Result<Value, String> {
    ingest(csv, receipt, dir, ROWS.div_ceil(200))
}
fn ingest(csv: &Path, receipt: &Path, dir: &Path, allowance: usize) -> Result<Value, String> {
    let started = Instant::now();
    let src = source(receipt)?;
    ingest_source(csv, src, dir, allowance, None, started)
}
#[derive(Clone, Copy)]
struct Slice {
    start_byte: u64,
    row_offset: usize,
    rows: usize,
}
fn verified_input(csv: &Path, src: &Value, cap: u64, started: Instant) -> Result<File, String> {
    let mut input = File::open(csv).map_err(|_| "CSV open failed")?;
    if !input
        .metadata()
        .map_err(|_| "CSV metadata failed")?
        .is_file()
    {
        return Err("regular CSV required".into());
    }
    let mut sha = Sha256::new();
    let mut size = 0u64;
    let mut buf = [0u8; 65536];
    loop {
        let n = input.read(&mut buf).map_err(|_| "CSV hash read failed")?;
        if n == 0 {
            break;
        }
        size += n as u64;
        if size > cap {
            return Err("CSV input cap".into());
        }
        deadline(started)?;
        sha.update(&buf[..n]);
    }
    if size != src["inputBytes"].as_u64().unwrap()
        || format!("{:x}", sha.finalize()) != src["inputSha256"].as_str().unwrap()
    {
        return Err("CSV size/hash mismatch".into());
    }
    use std::io::{Seek, SeekFrom};
    input
        .seek(SeekFrom::Start(0))
        .map_err(|_| "CSV rewind failed")?;
    Ok(input)
}
fn ingest_source(
    csv: &Path,
    src: Value,
    dir: &Path,
    allowance: usize,
    slice: Option<Slice>,
    started: Instant,
) -> Result<Value, String> {
    use std::io::{Seek, SeekFrom};
    let fingerprint = hash(&encode(&match slice {
        Some(s) => {
            json!({"source":src,"startByte":s.start_byte,"rowOffset":s.row_offset,"rows":s.rows})
        }
        None => src.clone(),
    })?);
    let input = verified_input(
        csv,
        &src,
        if slice.is_some() {
            segments::PARENT_BYTES
        } else {
            INPUT_CAP
        },
        started,
    )?;
    fs::create_dir_all(dir).map_err(|_| "output directory failed")?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join("usgwd-runner.lock"))
        .map_err(|_| "writer lock open failed")?;
    lock.try_lock().map_err(|_| "USGWD writer already active")?;
    let cp = dir.join("checkpoint-usgwd.json");
    let prior = if cp.exists() {
        let e =
            gev_us_groundwater::texas_sdr::decode_capture_json(&read_capped(&cp, CHECKPOINT_CAP)?)?;
        if e["schemaVersion"] != 1
            || e["sha256"] != hash(&encode(&e["payload"])?)
            || e["payload"]["sourceFingerprint"] != fingerprint
        {
            return Err("USGWD checkpoint/source mismatch".into());
        }
        e["payload"].clone()
    } else {
        json!({"sourceFingerprint":fingerprint,"partitions":[],"inputRows":0,"snapshotFullyRead":false})
    };
    let old = prior["partitions"]
        .as_array()
        .ok_or("checkpoint partition list")?;
    if old.len() > ROWS.div_ceil(200) {
        return Err("checkpoint partition cap".into());
    }
    let mut reader = BufReader::new(DigestReader {
        file: input,
        sha: Sha256::new(),
        bytes: 0,
    });
    let mut header =
        text(csv_row(&mut reader)?.ok_or("CSV header missing")?).map_err(str::to_owned)?;
    if let Some(first) = header.first_mut() {
        *first = first.trim_start_matches('\u{feff}').into();
    }
    if header != HEADER {
        return Err("USGWD schema/header changed".into());
    }
    let row_offset = slice.map_or(0, |s| s.row_offset);
    let read_start = slice.map_or(0, |s| s.start_byte);
    if read_start != 0 {
        let mut input = reader.into_inner().file;
        input
            .seek(SeekFrom::Start(read_start))
            .map_err(|_| "CSV slice seek failed")?;
        reader = BufReader::new(DigestReader {
            file: input,
            sha: Sha256::new(),
            bytes: 0,
        });
    }
    let (mut count, mut used, mut new_parts, mut spatial) = (0usize, 0u64, 0usize, 0u64);
    let parse_started = Instant::now();
    let (mut replayed_rows, mut replay_seconds) = (0usize, 0.0f64);
    let mut reports = sdr_jobs::Reports::default();
    let mut refs = Vec::new();
    loop {
        let mut records = Vec::new();
        let mut quarantine = Vec::new();
        let first = row_offset + count + 2;
        for _ in 0..200 {
            deadline(started)?;
            let Some(raw) = csv_row(&mut reader)? else {
                break;
            };
            count += 1;
            if count > ROWS {
                return Err("USGWD row cap; checkpoint retained".into());
            }
            // A bad byte in another column must not erase a recoverable ID:
            // invalid versions still poison every version of that release ID.
            let id = raw
                .first()
                .and_then(|f| std::str::from_utf8(f).ok())
                .filter(|s| native(s))
                .map(|s| format!("USGWD-{}-{s}", src["releaseSha256"].as_str().unwrap()));
            let fields = text(raw);
            match fields.and_then(|f| normalized(&f, &src)) {
                Ok(mut r) => {
                    let content = hash(&encode(&r)?);
                    reports.observe(id, Some(content.clone()), r["mapEligible"] == true);
                    spatial += u64::from(r["spatial"] == true);
                    r["contentSha256"] = json!(content);
                    r["provenance"] = json!({"inputSha256":src["inputSha256"],"sourceRow":row_offset+count+1,"releaseSha256":src["releaseSha256"]});
                    records.push(r);
                }
                Err(reason) => {
                    reports.observe(id.clone(), None, false);
                    quarantine.push(
                        json!({"sourceRow":row_offset+count+1,"sourceId":id,"reason":reason}),
                    );
                }
            }
        }
        if row_offset + count + 2 == first {
            break;
        }
        let value = json!({"schemaVersion":1,"kind":"rainfolk-usgwd-row-partition","source":src,"firstRow":first,"lastRow":row_offset+count+1,"records":records,"quarantine":quarantine,"requiresGlobalReconciliation":true});
        let index = refs.len();
        let bytes = encode(&value)?;
        let reference = if index < old.len() {
            let expected = json!({"path":format!("usgwd-part-{}.json",hash(&bytes)),"sha256":hash(&bytes),"bytes":bytes.len()});
            if old[index] != expected
                || read_capped(&dir.join(expected["path"].as_str().unwrap()), 8 * MAX_BYTES)?
                    != bytes
            {
                return Err("USGWD saved partition mismatch".into());
            }
            used += bytes.len() as u64;
            replayed_rows = count;
            replay_seconds = parse_started.elapsed().as_secs_f64();
            if used + OUTPUT_RESERVE > OUTPUT_CAP {
                return Err("USGWD replay/output cap".into());
            }
            expected
        } else {
            new_parts += 1;
            part(dir, "usgwd-part", &value, &mut used, started)?
        };
        refs.push(reference);
        if index >= old.len() {
            let mut state = json!({"sourceFingerprint":fingerprint,"partitions":refs,"inputRows":count,"snapshotFullyRead":false,"generation":index});
            save_bounded(&cp, &mut state, CHECKPOINT_CAP, started)?;
        }
        if slice.is_some_and(|s| count == s.rows) {
            break;
        }
        if new_parts >= allowance
            && !reader
                .fill_buf()
                .map_err(|_| "CSV read failure")?
                .is_empty()
        {
            if count >= ROWS {
                return Err("USGWD row cap; checkpoint retained".into());
            }
            return Ok(
                json!({"accounting":reports.counts(),"complete":false,"snapshotFullyRead":false,"partitions":refs.len(),"newPartitions":new_parts,"networkRequests":0,"prefixReplayRows":replayed_rows,"prefixReplaySeconds":replay_seconds}),
            );
        }
    }
    let eof = reader
        .fill_buf()
        .map_err(|_| "CSV read failure")?
        .is_empty();
    let end_byte = read_start + reader.get_ref().bytes - reader.buffer().len() as u64;
    let digest = reader.into_inner();
    if slice.is_some() {
        verified_input(csv, &src, segments::PARENT_BYTES, started)?;
    } else if digest.bytes != src["inputBytes"].as_u64().unwrap()
        || format!("{:x}", digest.sha.finalize()) != src["inputSha256"].as_str().unwrap()
    {
        return Err("CSV changed during processing".into());
    }
    if refs.len() < old.len() || (refs.len() == old.len() && prior["inputRows"] != count) {
        return Err("checkpoint source-row mismatch".into());
    }
    let mut indices = Vec::new();
    let mut chunk = Vec::new();
    for (id, (digest, n, mapped)) in &reports.ids {
        deadline(started)?;
        chunk.push(json!({"id":id,"contentSha256":digest,"sourceRows":n,"status":if digest.is_some(){"accepted"}else{"quarantined"},"mapEligible":digest.is_some() && *mapped}));
        if chunk.len() == 200 {
            indices.push(part(
                dir,
                "usgwd-index",
                &json!({"schemaVersion":1,"kind":"rainfolk-usgwd-id-index","ids":chunk}),
                &mut used,
                started,
            )?);
            chunk.clear();
        }
    }
    if !chunk.is_empty() {
        indices.push(part(
            dir,
            "usgwd-index",
            &json!({"schemaVersion":1,"kind":"rainfolk-usgwd-id-index","ids":chunk}),
            &mut used,
            started,
        )?);
    }
    let mut manifest = json!({"schemaVersion":1,"kind":"rainfolk-usgwd-manifest","source":src,"partitions":refs,"idIndex":indices,"accounting":reports.counts(),"spatialSourceRowsBeforeDeduplication":spatial,"complete":false,"snapshotFullyRead":true,"activation":"local-review-only","existingIndexEligible":false});
    if let Some(s) = slice {
        manifest["kind"] = json!("rainfolk-usgwd-segment");
        manifest["snapshotFullyRead"] = json!(eof);
        manifest["segmentFullyRead"] = json!(true);
        manifest["slice"] = json!({"startByte":s.start_byte,"endByte":end_byte,"rowOffset":row_offset,"rows":count});
    }
    deadline(started)?;
    if envelope_bytes(&manifest)? > MANIFEST_CAP
        || used + envelope_bytes(&manifest)? as u64 + CHECKPOINT_CAP as u64 > OUTPUT_CAP
    {
        return Err("USGWD manifest/output cap".into());
    }
    let final_path = dir.join("usgwd-manifest.json");
    if final_path.exists() {
        let previous = gev_us_groundwater::texas_sdr::decode_capture_json(&read_capped(
            &final_path,
            MANIFEST_CAP,
        )?)?;
        let mut payload = previous["payload"].clone();
        payload
            .as_object_mut()
            .ok_or("invalid manifest")?
            .remove("generation");
        if previous["sha256"] != hash(&encode(&previous["payload"])?) || payload != manifest {
            return Err("USGWD completed manifest mismatch".into());
        }
    } else {
        save_bounded(&final_path, &mut manifest, MANIFEST_CAP, started)?;
    }
    Ok(
        json!({"accounting":reports.counts(),"complete":false,"snapshotFullyRead":eof,"partitions":refs.len(),"newPartitions":new_parts,"manifestSha256":hash(&read_capped(&final_path,MANIFEST_CAP)?),"artifactBytes":used,"elapsedSeconds":started.elapsed().as_secs_f64(),"networkRequests":0,"prefixReplayRows":replayed_rows,"prefixReplaySeconds":replay_seconds,
        "limits":{"inputBytes":INPUT_CAP,"rows":ROWS,"outputBytes":OUTPUT_CAP,"checkpointBytes":CHECKPOINT_CAP,"manifestBytes":MANIFEST_CAP,"invocationSeconds":RUN_SECONDS}}),
    )
}

#[path = "usgwd_segments.rs"]
pub(super) mod segments;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn maximum_reference_counts_fit_readable_checkpoint_and_manifest_budgets() {
        let reference = json!({"path":"x".repeat(128),"sha256":"f".repeat(64),"bytes":8*MAX_BYTES});
        let refs = vec![reference; ROWS.div_ceil(200)];
        let cp = json!({"sourceFingerprint":"f".repeat(64),"partitions":refs,"inputRows":ROWS,"snapshotFullyRead":false,"generation":ROWS.div_ceil(200)});
        let manifest = json!({"source":"x".repeat(16384),"partitions":refs,"idIndex":refs,"accounting":{"inputRows":ROWS},"complete":false});
        assert!(envelope_bytes(&cp).unwrap() < CHECKPOINT_CAP);
        assert!(envelope_bytes(&manifest).unwrap() < MANIFEST_CAP);
        assert_eq!(OUTPUT_RESERVE, (CHECKPOINT_CAP + MANIFEST_CAP) as u64);
    }
    #[test]
    fn metadata_and_finalization_guards_fail_without_replacing_last_checkpoint() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../.temp-gev")
            .join(format!(
                "usgwd-budget-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("checkpoint.json");
        save_bounded(&path, &mut json!({"valid":true}), 256, Instant::now()).unwrap();
        let good = fs::read(&path).unwrap();
        assert!(
            save_bounded(
                &path,
                &mut json!({"oversized":"x".repeat(512)}),
                256,
                Instant::now()
            )
            .is_err()
        );
        assert_eq!(fs::read(&path).unwrap(), good);
        let expired = Instant::now() - Duration::from_secs(RUN_SECONDS + 1);
        assert!(save_bounded(&path, &mut json!({"late":true}), 256, expired).is_err());
        let mut used = 0;
        assert!(part(&dir, "late-index", &json!({}), &mut used, expired).is_err());
        assert_eq!(used, 0);
        assert_eq!(fs::read(&path).unwrap(), good);
        used = OUTPUT_CAP - OUTPUT_RESERVE;
        assert!(part(&dir, "full-index", &json!({}), &mut used, Instant::now()).is_err());
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        let scratch = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../.temp-gev")
            .canonicalize()
            .unwrap();
        assert!(dir.canonicalize().unwrap().starts_with(scratch));
        fs::remove_dir_all(&dir).unwrap();
    }
    #[test]
    fn csv_quotes_multiline_empty_fields_and_truncation() {
        let mut r = std::io::Cursor::new(b"a,\"b,\"\"c\"\"\n\",\r\nlast,,end");
        assert_eq!(
            text(csv_row(&mut r).unwrap().unwrap()).unwrap(),
            ["a", "b,\"c\"\n", ""]
        );
        assert_eq!(
            text(csv_row(&mut r).unwrap().unwrap()).unwrap(),
            ["last", "", "end"]
        );
        assert!(csv_row(&mut r).unwrap().is_none());
        for b in [
            b"\"unterminated".as_slice(),
            b"a\"bad",
            b"\"ok\"wrong",
            b"a\rb",
        ] {
            assert!(csv_row(&mut std::io::Cursor::new(b)).is_err());
        }
        assert!(csv_row(&mut std::io::Cursor::new(vec![b'a'; ROW_CAP + 1])).is_err());
    }
    #[test]
    fn normalization_keeps_release_identity_crs_strings_flags_and_unknowns() {
        let mut f = vec![String::new(); 29];
        f[0] = "NH_1".into();
        f[5] = "New Hampshire".into();
        f[6] = "00123".into();
        f[9] = "000123456789".into();
        f[11] = "Unknown".into();
        f[12] = "2".into();
        f[13] = "0".into();
        f[14] = "0".into();
        f[28] = "3".into();
        f[22] = "Unknown".into();
        let s = json!({"state":"New Hampshire","releaseSha256":"a".repeat(64)});
        let r = normalized(&f, &s).unwrap();
        assert_eq!(r["fips"], "00123");
        assert_eq!(r["huc12"], "000123456789");
        assert_eq!(r["coordinateReference"], "EPSG:4269");
        assert_eq!(r["mapEligible"], false);
        assert!(r["stateRecordId"].is_null());
        assert!(r["sourceYears"]["Year Well was Constructed"].is_null());
        assert_eq!(r["sourceValues"]["Flag Duplicate"], "3");
        f[2] = "-71.5".into();
        f[3] = "43.2".into();
        f[15] = "0".into();
        assert_eq!(normalized(&f, &s).unwrap()["mapEligible"], true);
        f[15] = "-1".into();
        assert!(normalized(&f, &s).is_err());
    }
}
