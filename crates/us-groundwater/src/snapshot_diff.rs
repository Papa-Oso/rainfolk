//! Full-snapshot, source-record comparison. No acquisition, deletion or activation.
use super::*;
use gev_us_groundwater::usgwd_map::Input;
use std::collections::{BTreeMap, BTreeSet};

struct Row {
    digest: String,
    state_id: Value,
    source_row: u64,
}
struct Snapshot {
    source: Value,
    manifest: String,
    rows: BTreeMap<String, Row>,
    quarantine: BTreeSet<String>,
    accounting: Value,
}
fn check(start: Instant, seconds: u64) -> Result<(), String> {
    if start.elapsed().as_secs() >= seconds {
        Err("snapshot diff deadline".into())
    } else {
        Ok(())
    }
}
fn load(path: &Path, start: Instant, seconds: u64) -> Result<Snapshot, String> {
    let mut input = Input::open_with_seconds(path, start, seconds)?;
    let quarantine = input.quarantined.keys().cloned().collect();
    let mut rows = BTreeMap::new();
    let mut anonymous = 0u64;
    let mut total = 0u64;
    for reference in input.candidates.clone() {
        check(start, seconds)?;
        let part = input.partition(&reference)?;
        if part["source"] != input.source {
            return Err("diff partition source mismatch".into());
        }
        let records = part["records"].as_array().ok_or("diff records")?;
        let bad = part["quarantine"].as_array().ok_or("diff quarantine")?;
        let count = records.len() + bad.len();
        if count > 200
            || part["firstRow"] != total + 2
            || part["lastRow"] != total + count as u64 + 1
        {
            return Err("diff row interval".into());
        }
        let mut ordinals: Vec<u64> = records
            .iter()
            .map(|r| r["provenance"]["sourceRow"].as_u64().unwrap_or(0))
            .chain(bad.iter().map(|r| r["sourceRow"].as_u64().unwrap_or(0)))
            .collect();
        ordinals.sort();
        if ordinals != (total + 2..=total + count as u64 + 1).collect::<Vec<_>>() {
            return Err("diff row conservation".into());
        }
        total += count as u64;
        for r in records {
            let id = r["id"].as_str().ok_or("diff identity")?;
            let mut content = r.clone();
            let obj = content.as_object_mut().ok_or("diff row object")?;
            obj.remove("provenance");
            obj.remove("contentSha256");
            let digest = hash(&encode(&content)?);
            if r["contentSha256"] != digest
                || r["provenance"]["inputSha256"] != input.source["inputSha256"]
                || r["provenance"]["releaseSha256"] != input.source["releaseSha256"]
            {
                return Err("diff content/provenance mismatch".into());
            }
            if let Some((expected, remaining, eligible)) = input.accepted.get_mut(id) {
                if *expected != digest || r["mapEligible"] != *eligible {
                    return Err("diff accepted index mismatch".into());
                }
                *remaining = remaining.checked_sub(1).ok_or("diff excess occurrence")?;
                rows.entry(id.into()).or_insert(Row {
                    digest,
                    state_id: r["stateRecordId"].clone(),
                    source_row: r["provenance"]["sourceRow"]
                        .as_u64()
                        .ok_or("diff source row")?,
                });
            } else {
                let n = input
                    .quarantined
                    .get_mut(id)
                    .ok_or("diff row outside index")?;
                *n = n.checked_sub(1).ok_or("diff quarantine excess")?;
            }
        }
        for r in bad {
            if let Some(id) = r["sourceId"].as_str() {
                let n = input
                    .quarantined
                    .get_mut(id)
                    .ok_or("diff quarantine outside index")?;
                *n = n.checked_sub(1).ok_or("diff quarantine excess")?;
            } else if r["sourceId"].is_null() {
                anonymous += 1;
            } else {
                return Err("diff quarantine identity".into());
            }
        }
    }
    if input.accepted.values().any(|(_, n, _)| *n != 0)
        || input.quarantined.values().any(|n| *n != 0)
        || anonymous != input.anonymous_quarantines
        || input.accounting["inputRows"] != total
    {
        return Err("diff index conservation".into());
    }
    Ok(Snapshot {
        source: input.source,
        manifest: input.manifest_sha256,
        rows,
        quarantine,
        accounting: input.accounting,
    })
}
fn evidence(snapshot: &Snapshot, row: &Row) -> Value {
    json!({"namespace":format!("USGWD/{}",snapshot.source["releaseSha256"].as_str().unwrap()),
        "rawStateRecordId":row.state_id,"releaseSha256":snapshot.source["releaseSha256"],
        "inputSha256":snapshot.source["inputSha256"],"sourceRow":row.source_row,
        "normalizationManifestSha256":snapshot.manifest,"contentSha256":row.digest,
        "aliasResolution":"unresolved; raw state ID is not a source-system crosswalk"})
}
pub(super) fn run(
    previous: Option<&Path>,
    current: &Path,
    dir: &Path,
    seconds: u64,
) -> Result<Value, String> {
    if !(1..=900).contains(&seconds) {
        return Err("diff deadline must be1..900".into());
    }
    let start = Instant::now();
    let old = previous.map(|p| load(p, start, seconds)).transpose()?;
    let new = load(current, start, seconds)?;
    if old.as_ref().is_some_and(|s| {
        s.source["state"] != new.source["state"]
            || s.source["resourceId"] != new.source["resourceId"]
    }) {
        return Err("diff source/state mismatch".into());
    }
    fs::create_dir_all(dir).map_err(|_| "diff output directory")?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join("runner.lock"))
        .map_err(|_| "diff lock")?;
    lock.try_lock().map_err(|_| "diff concurrent writer")?;
    let mut counts = BTreeMap::from([
        ("added", 0u64),
        ("changed", 0),
        ("missing", 0),
        ("quarantined", 0),
        ("unchanged", 0),
    ]);
    let mut refs = Vec::new();
    let mut pending = Vec::new();
    let mut used = 0usize;
    let mut ids: BTreeSet<&str> = new.rows.keys().map(String::as_str).collect();
    if let Some(s) = &old {
        ids.extend(s.rows.keys().map(String::as_str));
    }
    let flush =
        |pending: &mut Vec<Value>, refs: &mut Vec<Value>, used: &mut usize| -> Result<(), String> {
            if pending.is_empty() {
                return Ok(());
            }
            check(start, seconds)?;
            let bytes = encode(
                &json!({"schemaVersion":1,"kind":"rainfolk-snapshot-delta","events":pending}),
            )?;
            if bytes.len() > 8 * MAX_BYTES || *used + bytes.len() > 2 * 1024 * MAX_BYTES {
                return Err("diff output budget".into());
            }
            let name = immutable(dir, "refresh-delta", &bytes)?;
            *used += bytes.len();
            refs.push(
            json!({"path":name,"sha256":hash(&bytes),"bytes":bytes.len(),"events":pending.len()}),
        );
            pending.clear();
            Ok(())
        };
    for id in ids {
        check(start, seconds)?;
        let before = old.as_ref().and_then(|s| s.rows.get(id));
        let after = new.rows.get(id);
        let kind = match (before, after) {
            (Some(a), Some(b)) if a.digest == b.digest => "unchanged",
            (Some(_), Some(_)) => "changed",
            (None, Some(_)) => "added",
            (Some(_), None) if new.quarantine.contains(id) => "quarantined",
            _ => "missing",
        };
        *counts.get_mut(kind).unwrap() += 1;
        if kind == "unchanged" {
            continue;
        }
        pending.push(json!({"id":id,"kind":kind,"delete":false,"physicalWellStatus":null,
            "before":before.map(|r|evidence(old.as_ref().unwrap(),r)),"after":after.map(|r|evidence(&new,r))}));
        if pending.len() == 200
            || serde_json::to_vec(&pending)
                .map_err(|_| "diff encoding")?
                .len()
                >= 4 * MAX_BYTES
        {
            flush(&mut pending, &mut refs, &mut used)?;
        }
    }
    flush(&mut pending, &mut refs, &mut used)?;
    check(start, seconds)?;
    let report = json!({"schemaVersion":1,"kind":"rainfolk-snapshot-diff","comparison":"complete snapshots; not incremental source fetching",
        "sourceState":new.source["state"],"before":old.as_ref().map(|s|json!({"source":s.source,"manifestSha256":s.manifest,"accounting":s.accounting})),
        "after":{"source":new.source,"manifestSha256":new.manifest,"accounting":new.accounting},"counts":counts,"deltas":refs,
        "identity":"release-scoped source records; no proximity merge or automatic state-ID alias upsert",
        "requiresReview":counts["missing"]>0 || counts["quarantined"]>0 || new.accounting["quarantinedRows"].as_u64().unwrap_or(0)>0 || old.as_ref().is_some_and(|s|s.source["releaseSha256"]!=new.source["releaseSha256"]),
        "deletionPolicy":"retain history; missing is not plugged or permission to unpublish","affectedExportPartition":new.source["state"],"activation":"manual-only"});
    let bytes = encode(&report)?;
    if bytes.len() > 16 * MAX_BYTES || used + bytes.len() > 2 * 1024 * MAX_BYTES {
        return Err("diff report budget".into());
    }
    let name = immutable(dir, "refresh-report", &bytes)?;
    check(start, seconds)?;
    Ok(
        json!({"path":name,"sha256":hash(&bytes),"bytes":bytes.len(),"counts":counts,"requiresReview":report["requiresReview"]}),
    )
}
