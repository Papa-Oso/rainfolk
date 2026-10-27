//! Offline bulk stream admission using existing immutable/atomic job helpers.
use super::*;
use gev_us_groundwater::texas_sdr::{BULK_FIELDS, BULK_SOURCE, bulk_record};
use std::io::BufRead;
const MAX_ROWS: u64 = 1_000_000;
const OUTPUT_CAP: u64 = 4 * 1024 * 1024 * 1024;

fn line<R: BufRead>(r: &mut R) -> Result<Option<Value>, String> {
    let mut bytes = Vec::new();
    loop {
        let available = r.fill_buf().map_err(|_| "bulk input read")?;
        if available.is_empty() {
            if bytes.is_empty() {
                return Ok(None);
            } else {
                return Err("truncated bulk JSONL line".into());
            }
        }
        let n = available
            .iter()
            .position(|b| *b == b'\n')
            .map(|n| n + 1)
            .unwrap_or(available.len());
        if bytes.len() + n > MAX_BYTES {
            return Err("bulk input line cap".into());
        }
        let end = available[n - 1] == b'\n';
        bytes.extend_from_slice(&available[..n]);
        r.consume(n);
        if end {
            break;
        }
    }
    gev_us_groundwater::texas_sdr::decode_capture_json(&bytes).map(Some)
}
fn hex(v: &Value) -> bool {
    v.as_str()
        .is_some_and(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
}
fn keys(value: &Value, allowed: &[&str]) -> Result<(), String> {
    if value
        .as_object()
        .is_none_or(|o| o.keys().any(|k| !allowed.contains(&k.as_str())))
    {
        return Err("unapproved bulk envelope fields".into());
    }
    Ok(())
}
fn write_part(dir: &Path, kind: &str, value: &Value, used: &mut u64) -> Result<Value, String> {
    let raw = encode(value)?;
    if raw.len() > 16 * MAX_BYTES || *used + raw.len() as u64 > OUTPUT_CAP {
        return Err("bulk artifact/output byte cap".into());
    }
    let name = immutable(dir, kind, &raw)?;
    *used += raw.len() as u64;
    Ok(json!({"path":name,"sha256":hash(&raw),"bytes":raw.len()}))
}
pub(super) fn ingest<R: BufRead>(dir: &Path, input: &mut R) -> Result<Value, String> {
    let header = line(input)?.ok_or("bulk header missing")?;
    keys(
        &header,
        &[
            "schemaVersion",
            "kind",
            "sourceUrl",
            "archiveSha256",
            "archiveBytes",
            "member",
            "memberBytes",
            "headerSha256",
            "retrievedAt",
            "lastModified",
            "fields",
        ],
    )?;
    if header["schemaVersion"] != 1
        || header["kind"] != "rainfolk-texas-sdr-bulk-stream"
        || header["sourceUrl"] != BULK_SOURCE
        || header["member"] != "SDRDownload/WellData.txt"
        || header["fields"] != json!(BULK_FIELDS)
        || !hex(&header["archiveSha256"])
        || !hex(&header["headerSha256"])
        || header["archiveBytes"]
            .as_u64()
            .is_none_or(|n| n == 0 || n > 512 * MAX_BYTES as u64)
        || header["memberBytes"]
            .as_u64()
            .is_none_or(|n| n == 0 || n > 512 * MAX_BYTES as u64)
        || header["retrievedAt"]
            .as_str()
            .and_then(gev_us_groundwater::time_precision)
            != Some("instant")
    {
        return Err("invalid bulk source/projection header".into());
    }
    fs::create_dir(dir).map_err(|_| "fresh bulk output directory required")?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(dir.join("runner.lock"))
        .map_err(|_| "bulk writer lock")?;
    lock.try_lock().map_err(|_| "bulk writer already active")?;
    let (mut count, mut output_bytes) = (0, 0);
    let mut parts = Vec::new();
    let mut reports = sdr_jobs::Reports::default();
    let mut reasons = std::collections::BTreeMap::<String, u64>::new();
    let mut suppressed = std::collections::BTreeMap::<String, u64>::new();
    let terminal = loop {
        let batch = line(input)?.ok_or("bulk stream ended without verified EOF")?;
        if batch["kind"] == "end" {
            keys(&batch, &["kind", "rows", "memberBytes", "memberSha256"])?;
            break batch;
        }
        keys(&batch, &["kind", "rows"])?;
        let rows = batch["rows"].as_array().ok_or("bulk rows missing")?;
        if batch["kind"] != "batch"
            || rows.is_empty()
            || rows.len() > 200
            || count + rows.len() as u64 > MAX_ROWS
        {
            return Err("bulk batch/row cap".into());
        }
        let start = count + 2;
        let mut records = Vec::new();
        let mut quarantine = Vec::new();
        for row in rows {
            keys(row, &["row", "sha256", "id", "values", "error"])?;
            if row["row"].as_u64() != Some(count + 2)
                || !hex(&row["sha256"])
                || row.get("values").is_some() == row.get("error").is_some()
            {
                return Err("bulk source-row sequence/provenance mismatch".into());
            }
            let native = row["id"].as_str().filter(|s| {
                !s.is_empty() && s.len() <= 15 && s.bytes().all(|b| b.is_ascii_digit())
            });
            let id = native.map(|s| format!("TWDB-SDR-{s}"));
            let result = if let Some(error) = row["error"].as_str() {
                Err(match error {
                    "column-count" => "column-count",
                    "invalid-cp1252" => "invalid-cp1252",
                    "field-size" => "field-size",
                    _ => return Err("unapproved bulk reader error".into()),
                })
            } else {
                if native.is_some() && row["values"]["WellReportTrackingNumber"].as_str() != native
                {
                    return Err("bulk report ID projection mismatch".into());
                }
                bulk_record(&row["values"])
            };
            match result {
                Ok(mut record) => {
                    if record["id"].as_str() != id.as_deref() {
                        return Err("bulk accepted report ID provenance mismatch".into());
                    }
                    let content = hash(&encode(&record)?);
                    let mapped = record["mapEligible"] == true;
                    if !mapped {
                        *suppressed
                            .entry(record["mapSuppressionReason"].as_str().unwrap().into())
                            .or_default() += 1;
                    }
                    record["contentSha256"] = json!(content);
                    record["provenance"] = json!({"archiveSha256":header["archiveSha256"],"member":header["member"],"row":row["row"],"rawRowSha256":row["sha256"]});
                    reports.observe(id, Some(content), mapped);
                    records.push(record);
                }
                Err(reason) => {
                    reports.observe(id.clone(), None, false);
                    *reasons.entry(reason.into()).or_default() += 1;
                    quarantine.push(json!({"row":row["row"],"sourceId":id,"reason":reason,"rawRowSha256":row["sha256"]}));
                }
            }
            count += 1;
        }
        parts.push(write_part(dir,"sdr-bulk-part",&json!({"schemaVersion":1,"kind":"rainfolk-texas-sdr-bulk-row-partition",
            "source":header,"firstRow":start,"lastRow":count+1,"records":records,"quarantine":quarantine,
            "requiresGlobalReconciliation":true}),&mut output_bytes)?);
    };
    if terminal["rows"] != count
        || terminal["memberBytes"] != header["memberBytes"]
        || !hex(&terminal["memberSha256"])
        || line(input)?.is_some()
    {
        return Err("bulk EOF/hash/row accounting mismatch".into());
    }
    let mut indices = Vec::new();
    let mut chunk = Vec::new();
    for (id, (content, occurrences, mapped)) in &reports.ids {
        chunk.push(json!({"id":id,"contentSha256":content,"sourceRows":occurrences,"mapEligible":content.is_some() && *mapped,
            "status":if content.is_some(){"accepted"}else{"quarantined-conflicting-or-invalid-id"}}));
        if chunk.len() == 200 {
            indices.push(write_part(
                dir,
                "sdr-bulk-index",
                &json!({"schemaVersion":1,"kind":"rainfolk-texas-sdr-bulk-id-index","ids":chunk}),
                &mut output_bytes,
            )?);
            chunk.clear();
        }
    }
    if !chunk.is_empty() {
        indices.push(write_part(
            dir,
            "sdr-bulk-index",
            &json!({"schemaVersion":1,"kind":"rainfolk-texas-sdr-bulk-id-index","ids":chunk}),
            &mut output_bytes,
        )?);
    }
    let accounting = reports.counts();
    if accounting["inputRows"] != count {
        return Err("bulk reconciliation mismatch".into());
    }
    let mut manifest = json!({"schemaVersion":1,"kind":"rainfolk-texas-sdr-bulk-manifest","source":header,
        "memberSha256":terminal["memberSha256"],"partitions":parts,"idIndex":indices,"accounting":accounting,
        "rowQuarantineReasons":reasons,"rowMapSuppressionReasons":suppressed,"complete":false,"snapshotFullyRead":true,
        "activation":"local-review-only","depth":"not-derived-from-core-table","recordMeaning":"construction reports; not verified unique physical wells"});
    if output_bytes + encode(&manifest)?.len() as u64 + 1024 > OUTPUT_CAP {
        return Err("bulk manifest/output cap".into());
    }
    save(&dir.join("bulk-manifest.json"), &mut manifest)?;
    let raw = read_capped(&dir.join("bulk-manifest.json"), 16 * MAX_BYTES)?;
    Ok(
        json!({"accounting":accounting,"complete":false,"snapshotFullyRead":true,"partitionCount":parts.len(),"idIndexCount":indices.len(),
        "memberSha256":terminal["memberSha256"],"manifestSha256":hash(&raw),"manifestBytes":raw.len(),"artifactBytesExcludingManifest":output_bytes,
        "rowQuarantineReasons":reasons,"rowMapSuppressionReasons":suppressed}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn input_and_output_byte_caps_fail_before_artifact_writes() {
        assert!(line(&mut std::io::Cursor::new(vec![b' '; MAX_BYTES + 1])).is_err());
        let mut used = OUTPUT_CAP;
        assert!(write_part(Path::new("unused"), "part", &json!({}), &mut used).is_err());
        assert_eq!(used, OUTPUT_CAP);
    }
}
