//! Local work units over one immutable CSV, followed by streaming ID reconciliation.
use super::*;
use std::collections::VecDeque;

pub(super) const PARENT_BYTES: u64 = 512 * 1024 * 1024;
const SEGMENT_ROWS: usize = 125_000;
const SEGMENTS: usize = 16;

fn payload(bytes: &[u8]) -> Result<Value, String> {
    let e = gev_us_groundwater::texas_sdr::decode_capture_json(bytes)?;
    if e["schemaVersion"] != 1 || e["sha256"] != hash(&encode(&e["payload"])?) {
        return Err("USGWD segment envelope mismatch".into());
    }
    Ok(e["payload"].clone())
}
fn artifact(dir: &Path, r: &Value, kind: &str, started: Instant) -> Result<Vec<u8>, String> {
    deadline(started)?;
    if !hex(&r["sha256"]) || r["path"] != format!("{kind}-{}.json", r["sha256"].as_str().unwrap()) {
        return Err("USGWD segment artifact reference".into());
    }
    let bytes = read_capped(&dir.join(r["path"].as_str().unwrap()), 8 * MAX_BYTES)?;
    if r["bytes"] != bytes.len() || r["sha256"] != hash(&bytes) {
        return Err("USGWD segment artifact mismatch".into());
    }
    Ok(bytes)
}
fn segment_manifest(dir: &Path, r: &Value, n: usize, src: &Value) -> Result<Value, String> {
    if r["directory"] != format!("segment-{n:02}")
        || r["path"] != "usgwd-manifest.json"
        || !hex(&r["sha256"])
    {
        return Err("USGWD segment manifest reference".into());
    }
    let b = read_capped(
        &dir.join(r["directory"].as_str().unwrap())
            .join("usgwd-manifest.json"),
        MANIFEST_CAP,
    )?;
    if r["sha256"] != hash(&b) || r["bytes"] != b.len() {
        return Err("USGWD saved segment mismatch".into());
    }
    let p = payload(&b)?;
    if p["source"] != *src
        || p["kind"] != "rainfolk-usgwd-segment"
        || p["segmentFullyRead"] != true
        || p["complete"] != false
        || p["existingIndexEligible"] != false
    {
        return Err("USGWD saved segment contract".into());
    }
    Ok(p)
}
struct Index {
    dir: PathBuf,
    refs: Vec<Value>,
    next: usize,
    pending: VecDeque<Value>,
    head: Option<Value>,
    last: String,
    rows: u64,
    anonymous: u64,
    expected: Value,
    accepted: u64,
    duplicates: u64,
    quarantined: u64,
    mapped: u64,
}
impl Index {
    fn advance(&mut self, started: Instant) -> Result<(), String> {
        if self.pending.is_empty() && self.next < self.refs.len() {
            let b = artifact(&self.dir, &self.refs[self.next], "usgwd-index", started)?;
            let v = gev_us_groundwater::texas_sdr::decode_capture_json(&b)?;
            let ids = v["ids"].as_array().ok_or("USGWD index rows")?;
            if ids.is_empty() || ids.len() > 200 {
                return Err("USGWD index shard row cap".into());
            }
            self.pending = ids.clone().into();
            self.next += 1;
        }
        self.head = self.pending.pop_front();
        if let Some(v) = &self.head {
            let id = v["id"]
                .as_str()
                .filter(|s| !s.is_empty() && s.len() <= 144)
                .ok_or("USGWD index ID")?;
            let n = v["sourceRows"]
                .as_u64()
                .filter(|n| *n > 0 && *n <= ROWS as u64)
                .ok_or("USGWD index occurrence count")?;
            if id <= self.last.as_str()
                || (!v["contentSha256"].is_null() && !hex(&v["contentSha256"]))
                || v["mapEligible"].as_bool().is_none()
                || v["status"]
                    != if v["contentSha256"].is_null() {
                        "quarantined"
                    } else {
                        "accepted"
                    }
            {
                return Err("USGWD index ordering/content contract".into());
            }
            self.last = id.into();
            self.rows += n;
            if self.rows > ROWS as u64 {
                return Err("USGWD segment indexed row cap".into());
            }
            if v["contentSha256"].is_null() {
                self.quarantined += n;
            } else {
                self.accepted += 1;
                self.duplicates += n - 1;
                self.mapped += u64::from(v["mapEligible"] == true);
            }
        }
        Ok(())
    }
}
fn reconcile(
    dir: &Path,
    refs: &[Value],
    manifests: &[Value],
    src: &Value,
    started: Instant,
) -> Result<Value, String> {
    let mut streams = Vec::new();
    let mut input_rows = 0;
    let mut spatial = 0;
    for (r, m) in refs.iter().zip(manifests) {
        let d = dir.join(r["directory"].as_str().unwrap());
        let parts = m["partitions"]
            .as_array()
            .ok_or("USGWD segment partitions")?;
        let indices = m["idIndex"].as_array().ok_or("USGWD segment indices")?;
        if parts.len() > ROWS.div_ceil(200) || indices.len() > ROWS.div_ceil(200) {
            return Err("USGWD segment reference cap".into());
        }
        let mut bytes = 0u64;
        let (mut anonymous, mut source_rows) = (0u64, 0u64);
        for p in parts {
            let b = artifact(&d, p, "usgwd-part", started)?;
            bytes += b.len() as u64;
            let v = gev_us_groundwater::texas_sdr::decode_capture_json(&b)?;
            let q = v["quarantine"]
                .as_array()
                .ok_or("USGWD segment quarantine")?;
            let records = v["records"].as_array().ok_or("USGWD segment records")?;
            if records.len() + q.len() > 200 {
                return Err("USGWD partition row cap".into());
            }
            source_rows += (records.len() + q.len()) as u64;
            anonymous += q.iter().filter(|r| r["sourceId"].is_null()).count() as u64;
        }
        for p in indices {
            bytes += p["bytes"].as_u64().ok_or("USGWD index bytes")?;
        }
        if bytes + OUTPUT_RESERVE > OUTPUT_CAP {
            return Err("USGWD segment output cap".into());
        }
        if m["accounting"]["inputRows"] != source_rows {
            return Err("USGWD segment partition row conservation".into());
        }
        input_rows += m["accounting"]["inputRows"]
            .as_u64()
            .ok_or("USGWD segment row accounting")?;
        spatial += m["spatialSourceRowsBeforeDeduplication"]
            .as_u64()
            .ok_or("USGWD segment spatial count")?;
        let mut stream = Index {
            dir: d,
            refs: indices.clone(),
            next: 0,
            pending: VecDeque::new(),
            head: None,
            last: String::new(),
            rows: 0,
            anonymous,
            expected: m["accounting"].clone(),
            accepted: 0,
            duplicates: 0,
            quarantined: anonymous,
            mapped: 0,
        };
        stream.advance(started)?;
        streams.push(stream);
    }
    let (mut accepted, mut duplicates, mut quarantine, mut mapped, mut used) =
        (0u64, 0u64, 0u64, 0u64, 0u64);
    let (mut rows, mut shards, mut groups) = (Vec::new(), Vec::new(), Vec::new());
    loop {
        deadline(started)?;
        let Some(id) = streams
            .iter()
            .filter_map(|s| s.head.as_ref().and_then(|h| h["id"].as_str()))
            .min()
            .map(str::to_owned)
        else {
            break;
        };
        let (mut content, mut n, mut eligible) = (None, 0u64, true);
        let mut first = true;
        for stream in &mut streams {
            if let Some(v) = &stream.head {
                if v["id"] == id {
                    let h = v["contentSha256"].as_str().map(str::to_owned);
                    if first {
                        content = h.clone();
                        first = false;
                    } else if content != h {
                        content = None;
                    }
                    n += v["sourceRows"].as_u64().unwrap();
                    eligible &= v["mapEligible"] == true;
                    stream.advance(started)?;
                }
            }
        }
        if content.is_some() {
            accepted += 1;
            duplicates += n - 1;
            mapped += u64::from(eligible);
        } else {
            quarantine += n;
        }
        rows.push(json!({"id":id,"contentSha256":content,"sourceRows":n,"status":if content.is_some(){"accepted"}else{"quarantined"},"mapEligible":content.is_some()&&eligible}));
        if rows.len() == 200 {
            shards.push(part(
                dir,
                "usgwd-index",
                &json!({"schemaVersion":1,"kind":"rainfolk-usgwd-id-index","ids":rows}),
                &mut used,
                started,
            )?);
            rows.clear();
            if shards.len() == ROWS.div_ceil(200) {
                groups.push(part(dir,"usgwd-index-group",&json!({"schemaVersion":1,"kind":"rainfolk-usgwd-id-index-group","idIndex":shards}),&mut used,started)?);
                shards.clear();
            }
        }
    }
    for s in &streams {
        if s.expected
            != json!({"inputRows":s.rows+s.anonymous,"acceptedRecords":s.accepted,"identicalDuplicateRows":s.duplicates,"quarantinedRows":s.quarantined,"mapEligibleRecords":s.mapped,"nonmappedRecords":s.accepted-s.mapped})
        {
            return Err("USGWD segment index/accounting mismatch".into());
        }
        quarantine += s.anonymous;
    }
    if input_rows != accepted + duplicates + quarantine {
        return Err("USGWD global row conservation".into());
    }
    if !rows.is_empty() {
        shards.push(part(
            dir,
            "usgwd-index",
            &json!({"schemaVersion":1,"kind":"rainfolk-usgwd-id-index","ids":rows}),
            &mut used,
            started,
        )?);
    }
    if !shards.is_empty() {
        groups.push(part(
            dir,
            "usgwd-index-group",
            &json!({"schemaVersion":1,"kind":"rainfolk-usgwd-id-index-group","idIndex":shards}),
            &mut used,
            started,
        )?);
    }
    let accounting = json!({"inputRows":input_rows,"acceptedRecords":accepted,"identicalDuplicateRows":duplicates,"quarantinedRows":quarantine,"mapEligibleRecords":mapped,"nonmappedRecords":accepted-mapped});
    let mut manifest = json!({"schemaVersion":1,"kind":"rainfolk-usgwd-segmented-manifest","source":src,"segments":refs,"idIndexGroups":groups,"accounting":accounting,"spatialSourceRowsBeforeDeduplication":spatial,"snapshotFullyRead":true,"complete":false,"activation":"local-review-only","existingIndexEligible":false});
    let path = dir.join("usgwd-segmented-manifest.json");
    if path.exists() {
        let mut old = payload(&read_capped(&path, MANIFEST_CAP)?)?;
        old.as_object_mut()
            .ok_or("USGWD manifest object")?
            .remove("generation");
        if old != manifest {
            return Err("USGWD completed segmented manifest mismatch".into());
        }
    } else {
        save_bounded(&path, &mut manifest, MANIFEST_CAP, started)?;
    }
    Ok(accounting)
}
pub(crate) fn run(
    csv: &Path,
    receipt: &Path,
    dir: &Path,
    rows: usize,
    allowance: Option<usize>,
) -> Result<Value, String> {
    let started = Instant::now();
    if rows == 0 || rows > SEGMENT_ROWS || rows % 200 != 0 {
        return Err("USGWD segment rows must be a multiple of200 in200..125000".into());
    }
    if allowance.is_some_and(|n| !(1..=100).contains(&n)) {
        return Err("USGWD new partition allowance must be1..100".into());
    }
    let src = source_with_cap(receipt, PARENT_BYTES)?;
    verified_input(csv, &src, PARENT_BYTES, started)?;
    fs::create_dir_all(dir).map_err(|_| "USGWD segmented directory")?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join("usgwd-segments.lock"))
        .map_err(|_| "USGWD segment lock")?;
    lock.try_lock()
        .map_err(|_| "USGWD segmented writer already active")?;
    let fp = hash(&encode(
        &json!({"source":src,"rowsPerSegment":rows,"maxSegments":SEGMENTS}),
    )?);
    let cp = dir.join("checkpoint-usgwd-segments.json");
    let mut checkpoint = if cp.exists() {
        payload(&read_capped(&cp, CHECKPOINT_CAP)?)?
    } else {
        json!({"sourceFingerprint":fp,"segments":[]})
    };
    if checkpoint["sourceFingerprint"] != fp {
        return Err("USGWD segmented checkpoint/source mismatch".into());
    }
    let mut refs = checkpoint["segments"]
        .as_array()
        .ok_or("USGWD segmented checkpoint refs")?
        .clone();
    if refs.len() > SEGMENTS {
        return Err("USGWD segment count cap".into());
    }
    let (mut manifests, mut offset, mut row_offset, mut eof) = (Vec::new(), 0u64, 0usize, false);
    for (n, r) in refs.iter().enumerate() {
        deadline(started)?;
        let m = segment_manifest(dir, r, n, &src)?;
        let s = &m["slice"];
        let count = s["rows"]
            .as_u64()
            .filter(|v| *v <= rows as u64)
            .ok_or("USGWD segment rows")? as usize;
        let end = s["endByte"].as_u64().ok_or("USGWD segment end byte")?;
        let ended = m["snapshotFullyRead"]
            .as_bool()
            .ok_or("USGWD segment EOF")?;
        if eof
            || s["startByte"] != offset
            || s["rowOffset"] != row_offset
            || end <= offset
            || end > src["inputBytes"].as_u64().unwrap()
            || m["accounting"]["inputRows"] != count
            || (!ended && count != rows)
            || (ended && end != src["inputBytes"].as_u64().unwrap())
        {
            return Err("USGWD segment continuity/accounting mismatch".into());
        }
        offset = end;
        row_offset += count;
        eof = ended;
        manifests.push(m);
    }
    let mut added = 0;
    if !eof {
        if refs.len() == SEGMENTS {
            return Err("USGWD sixteen-segment cap; previous checkpoints retained".into());
        }
        if !cp.exists() {
            save_bounded(&cp, &mut checkpoint, CHECKPOINT_CAP, started)?;
        }
        let name = format!("segment-{:02}", refs.len());
        let d = dir.join(&name);
        let result = ingest_source(
            csv,
            src.clone(),
            &d,
            allowance.unwrap_or(rows.div_ceil(200)),
            Some(Slice {
                start_byte: offset,
                row_offset,
                rows,
            }),
            started,
        )?;
        if result["manifestSha256"].is_null() {
            return Ok(
                json!({"snapshotFullyRead":false,"complete":false,"segments":refs.len(),"newSegments":0,"sourceRowsProcessed":row_offset,"activeSegmentAccounting":result["accounting"],"activeSegmentPrefixReplayRows":result["prefixReplayRows"],"networkRequests":0}),
            );
        }
        let b = read_capped(&d.join("usgwd-manifest.json"), MANIFEST_CAP)?;
        let r = json!({"directory":name,"path":"usgwd-manifest.json","sha256":hash(&b),"bytes":b.len()});
        let m = segment_manifest(dir, &r, refs.len(), &src)?;
        row_offset += m["accounting"]["inputRows"]
            .as_u64()
            .ok_or("USGWD segment rows")? as usize;
        eof = m["snapshotFullyRead"] == true;
        refs.push(r);
        manifests.push(m);
        added = 1;
        checkpoint["segments"] = json!(refs);
        save_bounded(&cp, &mut checkpoint, CHECKPOINT_CAP, started)?;
    }
    let accounting = if eof {
        reconcile(dir, &refs, &manifests, &src, started)?
    } else {
        Value::Null
    };
    Ok(
        json!({"snapshotFullyRead":eof,"complete":false,"accounting":accounting,"segments":refs.len(),"newSegments":added,"sourceRowsProcessed":row_offset,"networkRequests":0,"elapsedSeconds":started.elapsed().as_secs_f64(),"limits":{"parentBytes":PARENT_BYTES,"rowsPerSegment":rows,"segments":SEGMENTS,"inputBytesPerSegment":INPUT_CAP,"outputBytesPerSegment":OUTPUT_CAP,"reconciliationOutputBytes":OUTPUT_CAP,"checkpointBytes":CHECKPOINT_CAP,"manifestBytes":MANIFEST_CAP,"invocationSeconds":RUN_SECONDS}}),
    )
}
