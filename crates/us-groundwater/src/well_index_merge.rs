//! Compose existing state indexes without copying data or claiming physical uniqueness.
use super::*;
type Error = Box<dyn std::error::Error>;
fn read_pinned(dir: &Path, r: &Value, suffix: &str) -> Result<Value, Error> {
    let sha = r["sha256"].as_str().ok_or("merge reference hash")?;
    if sha.len() != 64
        || !sha.bytes().all(|b| b.is_ascii_hexdigit())
        || r["url"] != format!("{sha}.{suffix}")
    {
        return Err("merge reference path".into());
    }
    let n = r["bytes"]
        .as_u64()
        .filter(|n| *n <= 8 * 1024 * 1024)
        .ok_or("merge reference byte cap")?;
    let path = dir.join(r["url"].as_str().unwrap());
    if fs::metadata(&path)?.len() > 8 * 1024 * 1024 {
        return Err("merge actual reference byte cap".into());
    }
    let b = fs::read(path)?;
    if b.len() as u64 != n || hash(&b) != sha {
        return Err("merge reference integrity".into());
    }
    Ok(serde_json::from_slice(&b)?)
}
pub(super) fn run(input: &Path, out: &Path) -> Result<Value, Error> {
    if fs::metadata(input)?.len() > 1024 * 1024 {
        return Err("merge descriptor cap".into());
    }
    let d = gev_us_groundwater::texas_sdr::decode_capture_json(&fs::read(input)?)?;
    let refs = d["states"].as_array().ok_or("merge state references")?;
    if refs.is_empty() || refs.len() > 50 {
        return Err("merge state cap".into());
    }
    let _lock = IndexWriterLock::acquire(out)?;
    let budget = Budget::new();
    let sizes = [32.0_f64, 8.0, 2.0, 0.5, 0.125, 0.03125];
    let mut grids: Vec<BTreeMap<(i32, i32), (u64, f64, f64)>> =
        sizes.iter().map(|_| BTreeMap::new()).collect();
    let (mut total, mut catalog, mut source_rows, mut q, mut duplicate_rows) =
        (0u64, 0u64, 0u64, 0u64, 0u64);
    let (mut children, mut ranges, mut states) =
        (Vec::new(), Vec::new(), std::collections::BTreeSet::new());
    let mut fallback = Value::Null;
    let mut release = Value::Null;
    for r in refs {
        budget.check()?;
        let state = r["state"]
            .as_str()
            .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphabetic() || b == b' '))
            .ok_or("merge state name")?;
        if !states.insert(state.to_owned()) {
            return Err("duplicate merge state".into());
        }
        let directory = format!("states/{}", state.replace(' ', "_"));
        if r["path"] != format!("{directory}/index.json") {
            return Err("merge index path".into());
        }
        let dir = out.join(&directory);
        let path = dir.join("index.json");
        if fs::metadata(&path)?.len() > 8 * 1024 * 1024 {
            return Err("merge child index byte cap".into());
        }
        let b = fs::read(&path)?;
        if r["sha256"] != hash(&b) {
            return Err("merge child index integrity".into());
        }
        let i = gev_us_groundwater::texas_sdr::decode_capture_json(&b)?;
        if i["schemaVersion"] != 1
            || i["kind"] != "gev-well-spatial-index"
            || i["sourceKind"] != "USGWD"
            || i["sourceState"] != state
            || i["normalizationPolicy"] != gev_us_groundwater::usgwd_map::POLICY
            || i["coverageComplete"] != false
            || !i["distinctPhysicalWells"].is_null()
        {
            return Err("merge child contract".into());
        }
        let id = &i["identityRange"];
        let first = id["first"].as_str().ok_or("merge first source ID")?;
        let last = id["last"].as_str().ok_or("merge last source ID")?;
        if first > last || first.len() > 144 || last.len() > 144 {
            return Err("merge source ID range".into());
        }
        if release.is_null() {
            release = id["releaseSha256"].clone();
        } else if release != id["releaseSha256"] {
            return Err("merge baseline release mismatch".into());
        }
        let prefix = format!("USGWD-{}-", release.as_str().ok_or("merge release ID")?);
        if !first.starts_with(&prefix) || !last.starts_with(&prefix) {
            return Err("merge identity namespace".into());
        }
        ranges.push((first.to_owned(), last.to_owned()));
        let n = i["indexedRecords"].as_u64().ok_or("merge indexed count")?;
        let a = &i["catalogAccounting"];
        let accepted = a["acceptedRecords"].as_u64().ok_or("merge catalog count")?;
        let qr = a["quarantinedRows"]
            .as_u64()
            .ok_or("merge quarantine count")?;
        let dr = a["identicalDuplicateRows"]
            .as_u64()
            .ok_or("merge duplicate count")?;
        let nr = a["inputRows"].as_u64().ok_or("merge source count")?;
        if n > accepted
            || nr != accepted + qr + dr
            || nr > 2_000_000
            || i["unmappedRecords"].as_u64() != Some(accepted - n)
        {
            return Err("merge catalog conservation".into());
        }
        total += n;
        catalog += accepted;
        source_rows += nr;
        q += qr;
        duplicate_rows += dr;
        if source_rows > 20_000_000 {
            return Err("merge nationwide row cap".into());
        }
        let tiles = i["regions"].as_array().ok_or("merge regions")?;
        if tiles.len() > 40_000
            || tiles
                .iter()
                .map(|t| t["count"].as_u64().unwrap_or(u64::MAX))
                .try_fold(0u64, |a, n| a.checked_add(n))
                .ok_or("merge count overflow")?
                != n
        {
            return Err("merge detail conservation".into());
        }
        let agg = &i["aggregates"];
        if agg["schemaVersion"] != 2 || agg["kind"] != "gev-well-partitioned-counts" {
            return Err("merge aggregate format".into());
        }
        let overview = read_pinned(&dir, &agg["overview"], "counts.json")?;
        let mut levels = overview["levels"]
            .as_array()
            .ok_or("merge overview levels")?
            .clone();
        for level in agg["levels"].as_array().ok_or("merge fine levels")? {
            let chunks = level["chunks"].as_array().ok_or("merge chunks")?;
            if chunks.len() > 4000 {
                return Err("merge summary chunk cap".into());
            }
            let mut cells = Vec::new();
            for r in chunks {
                budget.check()?;
                let chunk = read_pinned(&dir, r, "counts.json")?;
                let ls = chunk["levels"].as_array().ok_or("merge chunk levels")?;
                if ls.len() != 1 || ls[0]["degrees"] != level["degrees"] {
                    return Err("merge chunk resolution".into());
                }
                cells.extend(
                    ls[0]["cells"]
                        .as_array()
                        .ok_or("merge chunk cells")?
                        .iter()
                        .cloned(),
                );
                if cells.len() > 2_000_000 {
                    return Err("merge level cell cap".into());
                }
            }
            levels.push(json!({"degrees":level["degrees"],"total":n,"cells":cells}));
        }
        if levels.len() != 6 {
            return Err("merge six grid levels required".into());
        }
        for (k, level) in levels.iter().enumerate() {
            if level["degrees"] != sizes[k] {
                return Err("merge grid resolution".into());
            }
            let mut conserved = 0u64;
            let mut seen = std::collections::BTreeSet::new();
            for c in level["cells"].as_array().ok_or("merge grid cells")? {
                budget.check()?;
                let bounds = c["bounds"]
                    .as_array()
                    .filter(|a| a.len() == 4)
                    .ok_or("merge cell bounds")?;
                let x = bounds[0]
                    .as_f64()
                    .filter(|v| v.is_finite())
                    .ok_or("merge cell longitude")?;
                let y = bounds[1]
                    .as_f64()
                    .filter(|v| v.is_finite())
                    .ok_or("merge cell latitude")?;
                let key = (
                    ((x + 180.0) / sizes[k]).round() as i32,
                    ((y + 90.0) / sizes[k]).round() as i32,
                );
                if !seen.insert(key) {
                    return Err("merge duplicate grid cell".into());
                }
                let count = c["count"]
                    .as_u64()
                    .filter(|n| *n > 0 && *n <= 2_000_000)
                    .ok_or("merge cell count")?;
                let center = &c["center"];
                let lon = center[0]
                    .as_f64()
                    .filter(|v| {
                        v.is_finite() && *v >= x && *v <= bounds[2].as_f64().unwrap_or(x - 1.0)
                    })
                    .ok_or("merge cell center")?;
                let lat = center[1]
                    .as_f64()
                    .filter(|v| {
                        v.is_finite() && *v >= y && *v <= bounds[3].as_f64().unwrap_or(y - 1.0)
                    })
                    .ok_or("merge cell center")?;
                let v = grids[k].entry(key).or_default();
                v.0 += count;
                v.1 += lon * count as f64;
                v.2 += lat * count as f64;
                conserved += count;
            }
            if conserved != n {
                return Err("merge aggregate conservation".into());
            }
        }
        if fallback.is_null() {
            fallback = i["fallbackAcquisition"].clone();
        }
        let name = format!("{}.index.json", hash(&b));
        write_immutable(&dir, &budget, &name, &b)?;
        let bounds = tiles
            .iter()
            .fold([180.0f64, 90.0, -180.0, -90.0], |mut b, t| {
                for axis in 0..2 {
                    b[axis] = b[axis].min(t["bounds"][axis].as_f64().unwrap_or(180.0));
                    b[axis + 2] = b[axis + 2].max(t["bounds"][axis + 2].as_f64().unwrap_or(-180.0));
                }
                b
            });
        if n > 0 {
            children.push(json!({"state":state,"url":format!("{directory}/{name}"),"directory":format!("{directory}/"),"sha256":hash(&b),"bytes":b.len(),"count":n,"bounds":bounds,"identityRange":id}));
        }
    }
    ranges.sort();
    if ranges.windows(2).any(|r| r[0].1 >= r[1].0) {
        return Err(
            "overlapping release-scoped ID ranges; explicit reconciliation required".into(),
        );
    }
    let levels:Vec<Value>=grids.iter().zip(sizes).map(|(grid,size)|json!({"degrees":size,"total":total,"cells":grid.iter().map(|(&(x,y),&(n,lon,lat))|json!({"bounds":[x as f64*size-180.0,y as f64*size-90.0,((x+1) as f64*size-180.0).min(180.0),((y+1) as f64*size-90.0).min(90.0)],"center":[lon/n as f64,lat/n as f64],"count":n})).collect::<Vec<_>>()})).collect();
    let aggregates = publish_partitioned_counts(out, &budget, &levels, total as usize)?;
    let index = json!({"schemaVersion":1,"kind":"gev-well-spatial-index","sourceKind":"USGWD","coverageComplete":false,"regions":[],"children":children,"aggregates":aggregates,"indexedRecords":total,"catalogRecords":catalog,"unmappedRecords":catalog-total,"sourceRows":source_rows,"quarantinedRows":q,"identicalDuplicateRows":duplicate_rows,"distinctPhysicalWells":null,"fallbackAcquisition":fallback,"snapshot":{"releaseSha256":release,"normalizationPolicy":gev_us_groundwater::usgwd_map::POLICY,"description":"Historical source records; physical-well identity unresolved. No cross-source proximity merge. Counts include mapped catalog records only.","blockedStates":d["blockedStates"]}});
    let b = serde_json::to_vec(&index)?;
    if b.len() > 8 * 1024 * 1024 {
        return Err("merge root byte cap".into());
    }
    budget.charge(b.len())?;
    if out.join("index.json").exists() && fs::read(out.join("index.json"))? == b {
        return Ok(
            json!({"states":states.len(),"indexedRecords":total,"catalogRecords":catalog,"indexSha256":hash(&b),"unchanged":true}),
        );
    }
    if out.join("index.json").exists() {
        let old = fs::read(out.join("index.json"))?;
        write_immutable(out, &budget, &format!("{}.index.json", hash(&old)), &old)?;
    }
    let pending = out.join("index.pending");
    fs::write(&pending, &b)?;
    fs::OpenOptions::new()
        .write(true)
        .open(&pending)?
        .sync_all()?;
    budget.check()?;
    fs::rename(&pending, out.join("index.json"))?;
    Ok(
        json!({"states":states.len(),"indexedRecords":total,"catalogRecords":catalog,"unmappedRecords":catalog-total,"indexSha256":hash(&b)}),
    )
}
