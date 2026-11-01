//! Explicit historical-record bridge into the existing count/detail index.
//! No acquisition, physical-well merge, measurement correction or activation.
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

pub const POLICY: &str = "usgwd-field-quality-v2";
const RESOURCE: &str = "8b02895f02c14dd1a749bcc5584a5c55";
const MAX_ROWS: u64 = 2_000_000;
pub fn digest(b: &[u8]) -> String {
    format!("{:x}", Sha256::digest(b))
}
fn hex(v: &Value) -> bool {
    v.as_str()
        .is_some_and(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
}
/// Receipt provenance is an operator assertion, as with the original CSV profile.
/// Derived DBF inputs retain the actual published ZIP URL and member identity.
pub fn source_url_valid(source: &Value) -> bool {
    let state = source["state"].as_str().unwrap_or("").replace(' ', "_");
    let base = format!("https://www.hydroshare.org/resource/{RESOURCE}/data/contents/");
    match source.get("derivation") {
        None => {
            source["schemaVersion"] == 1
                && source["sourceUrl"] == format!("{base}USGWD-Tabular/USGWD_{state}.csv")
        }
        Some(d) => {
            let fields = [
                "policy",
                "archiveSha256",
                "archiveMd5",
                "member",
                "memberSha256",
                "sourceCrs",
                "recordCount",
            ];
            source["schemaVersion"] == 2
                && source["sourceUrl"] == format!("{base}USGWD-Geospatial/USGWD_{state}.zip")
                && d.as_object().is_some_and(|o| {
                    o.len() == fields.len() && o.keys().all(|k| fields.contains(&k.as_str()))
                })
                && d["policy"] == "usgwd-dbf-attributes-v1"
                && d["sourceCrs"] == "EPSG:4269"
                && d["member"] == format!("USGWD_{}.dbf", source["state"].as_str().unwrap_or(""))
                && hex(&d["archiveSha256"])
                && hex(&d["memberSha256"])
                && d["archiveMd5"]
                    .as_str()
                    .is_some_and(|s| s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit()))
                && d["recordCount"]
                    .as_u64()
                    .is_some_and(|n| n > 0 && n <= MAX_ROWS)
        }
    }
}
fn check_time(start: Instant, seconds: u64) -> Result<(), String> {
    if start.elapsed() >= Duration::from_secs(seconds) {
        Err(format!("historical index {seconds}-second deadline"))
    } else {
        Ok(())
    }
}
fn read(path: &Path, cap: u64) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let mut b = Vec::new();
    fs::File::open(path)
        .map_err(|_| "historical input open")?
        .take(cap + 1)
        .read_to_end(&mut b)
        .map_err(|_| "historical input read")?;
    if b.len() as u64 > cap {
        return Err("historical input byte cap".into());
    }
    Ok(b)
}
fn json(b: &[u8]) -> Result<Value, String> {
    crate::texas_sdr::decode_capture_json(b)
}
fn envelope(b: &[u8]) -> Result<Value, String> {
    let e = json(b)?;
    if e["schemaVersion"] != 1
        || e["sha256"]
            != digest(
                &serde_json::to_vec(&e["payload"]).map_err(|_| "historical envelope encoding")?,
            )
    {
        return Err("historical envelope integrity".into());
    }
    Ok(e["payload"].clone())
}
fn artifact(dir: &Path, r: &Value, kind: &str) -> Result<Vec<u8>, String> {
    if !hex(&r["sha256"]) || r["path"] != format!("{kind}-{}.json", r["sha256"].as_str().unwrap()) {
        return Err("historical artifact reference".into());
    }
    let b = read(&dir.join(r["path"].as_str().unwrap()), 8 * 1024 * 1024)?;
    if r["bytes"] != b.len() || r["sha256"] != digest(&b) {
        return Err("historical artifact integrity".into());
    }
    Ok(b)
}
pub struct Input {
    pub source: Value,
    pub accounting: Value,
    pub manifest_sha256: String,
    pub candidates: Vec<Value>,
    pub accepted: BTreeMap<String, (String, u64, bool)>,
    pub quarantined: BTreeMap<String, u64>,
    pub anonymous_quarantines: u64,
    pub root: PathBuf,
}
impl Input {
    pub fn open(path: &Path, started: Instant) -> Result<Self, String> {
        Self::open_with_seconds(path, started, 300)
    }
    pub fn open_with_seconds(path: &Path, started: Instant, seconds: u64) -> Result<Self, String> {
        if !(1..=900).contains(&seconds) {
            return Err("historical deadline must be 1..900 seconds".into());
        }
        let b = read(path, 1024 * 1024)?;
        let m = envelope(&b)?;
        if m["normalizationPolicy"] != POLICY
            || m["snapshotFullyRead"] != true
            || m["complete"] != false
            || m["existingIndexEligible"] != false
        {
            return Err("historical manifest policy/completion".into());
        }
        let source = m["source"].clone();
        if source["resourceId"] != RESOURCE
            || source["license"] != "CC-BY-4.0"
            || !hex(&source["releaseSha256"])
            || !hex(&source["inputSha256"])
            || crate::time_precision(source["checkedAt"].as_str().unwrap_or("")) != Some("instant")
            || !source_url_valid(&source)
        {
            return Err("historical source contract".into());
        }
        let root = path
            .parent()
            .ok_or("historical manifest parent")?
            .to_path_buf();
        let mut parts = Vec::new();
        let mut indices = Vec::new();
        match m["kind"].as_str() {
            Some("rainfolk-usgwd-manifest") => {
                for r in m["partitions"].as_array().ok_or("historical partitions")? {
                    parts.push(r.clone())
                }
                for r in m["idIndex"].as_array().ok_or("historical index")? {
                    indices.push(r.clone())
                }
            }
            Some("rainfolk-usgwd-segmented-manifest") => {
                let segments = m["segments"].as_array().ok_or("historical segments")?;
                if segments.is_empty() || segments.len() > 16 {
                    return Err("historical segment cap".into());
                }
                for (n, r) in segments.iter().enumerate() {
                    check_time(started, seconds)?;
                    if r["directory"] != format!("segment-{n:02}")
                        || r["path"] != "usgwd-manifest.json"
                        || !hex(&r["sha256"])
                    {
                        return Err("historical segment reference".into());
                    }
                    let b = read(
                        &root
                            .join(r["directory"].as_str().unwrap())
                            .join("usgwd-manifest.json"),
                        1024 * 1024,
                    )?;
                    if r["bytes"] != b.len() || r["sha256"] != digest(&b) {
                        return Err("historical segment integrity".into());
                    }
                    let p = envelope(&b)?;
                    if p["source"] != source
                        || p["normalizationPolicy"] != POLICY
                        || p["kind"] != "rainfolk-usgwd-segment"
                        || p["segmentFullyRead"] != true
                    {
                        return Err("historical segment contract".into());
                    }
                    for part in p["partitions"]
                        .as_array()
                        .ok_or("historical segment partitions")?
                    {
                        let mut part = part.clone();
                        part["directory"] = r["directory"].clone();
                        parts.push(part);
                    }
                }
                for r in m["idIndexGroups"]
                    .as_array()
                    .ok_or("historical index groups")?
                {
                    check_time(started, seconds)?;
                    let g = json(&artifact(&root, r, "usgwd-index-group")?)?;
                    indices.extend(
                        g["idIndex"]
                            .as_array()
                            .ok_or("historical group indices")?
                            .iter()
                            .cloned(),
                    );
                    if indices.len() > 10_000 {
                        return Err("historical index reference cap".into());
                    }
                }
            }
            _ => return Err("historical manifest kind".into()),
        }
        if parts.len() > 10_016
            || indices.len() > 10_000
            || m["accounting"]["inputRows"]
                .as_u64()
                .is_none_or(|n| n > MAX_ROWS)
        {
            return Err("historical row/partition cap".into());
        }
        let mut accepted = BTreeMap::new();
        let mut quarantined = BTreeMap::new();
        let mut last = String::new();
        let (mut occurrences, mut q, mut duplicates, mut mapped) = (0u64, 0u64, 0u64, 0u64);
        for r in indices {
            check_time(started, seconds)?;
            let index = json(&artifact(&root, &r, "usgwd-index")?)?;
            let ids = index["ids"].as_array().ok_or("historical index ids")?;
            if ids.len() > 200 {
                return Err("historical index shard cap".into());
            }
            for id in ids {
                let key = id["id"].as_str().ok_or("historical index id")?;
                let n = id["sourceRows"]
                    .as_u64()
                    .filter(|n| *n > 0 && *n <= MAX_ROWS)
                    .ok_or("historical index occurrence")?;
                if key <= last.as_str()
                    || !key.starts_with(&format!(
                        "USGWD-{}-",
                        source["releaseSha256"].as_str().unwrap()
                    ))
                {
                    return Err("historical index ordering/namespace".into());
                }
                last = key.into();
                occurrences += n;
                match id["status"].as_str() {
                    Some("accepted")
                        if hex(&id["contentSha256"]) && id["mapEligible"].is_boolean() =>
                    {
                        accepted.insert(
                            key.into(),
                            (
                                id["contentSha256"].as_str().unwrap().into(),
                                n,
                                id["mapEligible"] == true,
                            ),
                        );
                        duplicates += n - 1;
                        mapped += u64::from(id["mapEligible"] == true);
                    }
                    Some("quarantined") if id["contentSha256"].is_null() => {
                        q += n;
                        quarantined.insert(key.into(), n);
                    }
                    _ => return Err("historical index status".into()),
                }
                if occurrences > MAX_ROWS {
                    return Err("historical index row cap".into());
                }
            }
        }
        let a = &m["accounting"];
        if a["acceptedRecords"] != accepted.len()
            || a["identicalDuplicateRows"] != duplicates
            || a["mapEligibleRecords"] != mapped
            || a["quarantinedRows"].as_u64().is_none_or(|n| n < q)
            || a["inputRows"].as_u64()
                != Some(accepted.len() as u64 + duplicates + a["quarantinedRows"].as_u64().unwrap())
        {
            return Err("historical index accounting".into());
        }
        Ok(Self {
            source,
            accounting: a.clone(),
            manifest_sha256: digest(&b),
            candidates: parts,
            accepted,
            quarantined,
            anonymous_quarantines: a["quarantinedRows"].as_u64().unwrap() - q,
            root,
        })
    }
    pub fn partition(&self, r: &Value) -> Result<Value, String> {
        let dir = match r["directory"].as_str() {
            Some(d) => self.root.join(d),
            None => self.root.clone(),
        };
        json(&artifact(&dir, r, "usgwd-part")?)
    }
}

/// EPSG 9603: 2D geocentric translations, h=0 intermediate only, discard h.
/// GRS80 -> WGS84 ellipsoids; longitude/latitude API order is explicit.
pub fn coordinate(lon: f64, lat: f64, state: &str) -> Option<(f64, f64, Value)> {
    if !lon.is_finite() || !lat.is_finite() {
        return None;
    }
    let (op, t, accuracy) = if state == "Hawaii"
        && (-160.31..=-154.75).contains(&lon)
        && (18.86..=22.30).contains(&lat)
    {
        (1252, [1.0, 1.0, -1.0], 4.0)
    } else if state == "Alaska"
        && (51.3..=54.34).contains(&lat)
        && (lon >= 172.42 && lon <= 180.0 || lon >= -180.0 && lon <= -164.84)
    {
        (1251, [-2.0, 0.0, 4.0], 8.0)
    } else if state != "Hawaii"
        && (-172.54..=-47.74).contains(&lon)
        && (23.81..=86.46).contains(&lat)
    {
        (1188, [0.0, 0.0, 0.0], 4.0)
    } else {
        return None;
    };
    let a = 6378137.0;
    let f = 1.0 / 298.257222101;
    let e = f * (2.0 - f);
    let p = lat.to_radians();
    let l = lon.to_radians();
    let n = a / (1.0 - e * p.sin().powi(2)).sqrt();
    let x = n * p.cos() * l.cos() + t[0];
    let y = n * p.cos() * l.sin() + t[1];
    let z = n * (1.0 - e) * p.sin() + t[2];
    let f2 = 1.0 / 298.257223563;
    let e2 = f2 * (2.0 - f2);
    let xy = x.hypot(y);
    let mut phi = z.atan2(xy * (1.0 - e2));
    for _ in 0..6 {
        let n2 = a / (1.0 - e2 * phi.sin().powi(2)).sqrt();
        phi = (z + e2 * n2 * phi.sin()).atan2(xy);
    }
    let result = (y.atan2(x).to_degrees(), phi.to_degrees());
    Some((
        result.0,
        result.1,
        json!({"sourceCrs":"EPSG:4269","targetCrs":"EPSG:4326","operation":format!("EPSG:{op}"),"method":"EPSG:9603","translationMetres":t,"operationAccuracyMetres":accuracy,"intermediateHeightMetres":0,"outputHeight":null,"epoch":null,"warning":"Approximate datum operation; source location accuracy remains unknown. Not a surveyed wellhead or survey transformation."}),
    ))
}

pub fn record(r: &Value, source: &Value) -> Result<Option<Value>, String> {
    if r["normalizationPolicy"] != POLICY
        || r["source"] != "USGWD"
        || r["coordinateReference"] != "EPSG:4269"
        || r["existingIndexEligible"] != false
        || r["sourceValues"].as_object().is_none_or(|v| v.len() != 29)
    {
        return Err("historical normalized record contract".into());
    }
    if r["mapEligible"] != true {
        return Ok(None);
    }
    let lon = r["longitude"].as_f64().ok_or("historical longitude")?;
    let lat = r["latitude"].as_f64().ok_or("historical latitude")?;
    let Some((x, y, operation)) = coordinate(
        lon,
        lat,
        source["state"].as_str().ok_or("historical state")?,
    ) else {
        return Ok(None);
    };
    let mut flags = serde_json::Map::new();
    for key in [
        "Location Verified",
        "Flag County",
        "Flag State",
        "Flag US",
        "Flag Duplicate",
    ] {
        flags.insert(key.into(), r["sourceValues"][key].clone());
    }
    let errors: Vec<Value> = r["quantityErrors"].as_array().ok_or("historical quantity errors")?.iter().map(|e|json!({"field":e["field"],"reason":e["reason"],"rawValue":r["sourceValues"][e["field"].as_str().unwrap_or("")]})).collect();
    Ok(Some(
        json!({"id":r["id"],"name":format!("Historical USGWD record {}; location unverified",r["sourceRecordId"].as_str().ok_or("historical native id")?),"source":"USGWD","sourceUrl":format!("https://www.hydroshare.org/resource/{RESOURCE}/"),"longitude":x,"latitude":y,"coordinateReference":"EPSG:4326","readings":[],"metadataRetrievedAt":source["checkedAt"],"metadataSourceUrl":source["sourceUrl"],"metadata":{"id":r["id"],"recordType":"historical-well-catalog-record","rawStateRecordId":r["stateRecordId"],"sourceRecordId":r["sourceRecordId"],"releaseSha256":source["releaseSha256"],"sourceState":source["state"],"identityResolution":"distinct source record; physical-well identity unresolved; no proximity merge","sourceCoordinates":r["sourceCoordinates"],"coordinateOperation":operation,"horizontal_positional_accuracy":"Source location accuracy unknown; approximate datum conversion; not a surveyed wellhead","locationFlags":flags,"quantities":r["quantities"],"quantityErrors":errors,"sourceYears":r["sourceYears"],"provenance":r["provenance"],"normalizedContentSha256":r["contentSha256"],"sourceFileModifiedAt":source["sourceFileModifiedAt"],"sourcePublishedAt":source["sourcePublishedAt"],"activeWellStatus":"not-verified","waterSafetyStatus":"not-assessed"}}),
    ))
}
pub fn pack(records: Vec<Value>, source: &Value, manifest_sha: &str) -> Value {
    let mut value = json!({"schemaVersion":1,"kind":"gev-us-groundwater-wells","mode":"snapshot","coverage":{"country":"US","complete":false,"description":"Historical USGWD source records, not a count of distinct physical wells. Locations unverified; no water-level observations or current activity/water-safety assessment."},"acquisitions":[{"collection":"usgwd-historical-baseline","url":source["sourceUrl"],"retrievedAt":source["checkedAt"],"releaseSha256":source["releaseSha256"],"inputSha256":source["inputSha256"],"normalizationManifestSha256":manifest_sha,"license":source["license"],"attribution":source["attribution"]}],"records":records});
    if let Some(derivation) = source.get("derivation") {
        value["acquisitions"][0]["derivation"] = derivation.clone();
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_input_deadline_remains_bounded() {
        let started = Instant::now() - Duration::from_secs(301);
        assert!(check_time(started, 300).is_err());
        assert!(check_time(started, 900).is_ok());
        for seconds in [0, 901, u64::MAX] {
            assert!(
                Input::open_with_seconds(Path::new("missing"), Instant::now(), seconds).is_err()
            );
        }
    }
    #[test]
    fn regional_operations_are_explicit_bounded_and_preserve_axis_order() {
        let (x, y, o) = coordinate(-71.5, 43.2, "New Hampshire").unwrap();
        assert!((x + 71.5).abs() < 1e-10 && (y - 43.2).abs() < 1e-8);
        assert_eq!(o["operation"], "EPSG:1188");
        let (x, y, o) = coordinate(-157.8, 21.3, "Hawaii").unwrap();
        assert_eq!(o["operation"], "EPSG:1252");
        assert!((x + 157.8).abs() > 1e-7 && (y - 21.3).abs() > 1e-7);
        assert_eq!(
            coordinate(175.0, 52.0, "Alaska").unwrap().2["operation"],
            "EPSG:1251"
        );
        for (x, y, s) in [
            (0.0, 0.0, "Florida"),
            (-157.8, 21.3, "Florida"),
            (-71.5, 43.2, "Hawaii"),
            (43.2, -71.5, "New Hampshire"),
            (f64::NAN, 43.2, "New Hampshire"),
        ] {
            assert!(coordinate(x, y, s).is_none());
        }
    }
}
