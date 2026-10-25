//! Explicit offline continuation; no acquisition or automatic renewal.
use super::*;
pub(crate) fn migrate(
    old: &Value,
    source: &Path,
    new: &Value,
    target: &Path,
    expiry: u64,
    recovery: Option<&Path>,
    now: u64,
) -> Result<Value, String> {
    window(old, new, expiry, now)?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .open(source.join("runner.lock"))
        .map_err(|e| e.to_string())?;
    lock.try_lock().map_err(|_| "source job is active")?;
    let parent = fs::read(source.join("checkpoint-v2.json")).map_err(|e| e.to_string())?;
    let (mut state, mut indices) = load(old, source)?;
    if state["networkBlocked"] == true || state["quotaBlocked"] == true {
        return Err("blocked source cannot be renewed".into());
    }
    // Refuse stale recovery requests before creating or copying a destination.
    if let Some(recovery) = recovery {
        for item in old["states"].as_array().unwrap() {
            let code = item["state"].as_str().unwrap();
            let recovery_path = recovery.join(format!("{code}-capture.json"));
            if recovery_path.exists() {
                let capture: Value =
                    serde_json::from_slice(&read_capped(&recovery_path, 8 * MAX_BYTES)?)
                        .map_err(|e| e.to_string())?;
                let pages = capture["acquisitions"]
                    .as_array()
                    .ok_or("missing recovery pages")?;
                if pages.len() != 1 {
                    return Err("recovery must be one reserved page".into());
                }
                let url = pages[0]["url"].as_str().ok_or("missing recovery URL")?;
                if indices[code].requests.contains(url)
                    || state["states"][code]["status"] != "failed"
                {
                    return Err(format!(
                        "recovery already integrated or no failed reserved attempt for {code}; nothing copied"
                    ));
                }
            }
        }
    }
    if target.exists() {
        return Err("destination must be fresh".into());
    }
    if target
        .parent()
        .ok_or("missing destination parent")?
        .canonicalize()
        .map_err(|e| e.to_string())?
        .starts_with(source.canonicalize().map_err(|e| e.to_string())?)
    {
        return Err("destination cannot be inside source".into());
    }
    fn copy(source: &Path, target: &Path) -> Result<(), String> {
        fs::create_dir(target).map_err(|e| e.to_string())?;
        for e in fs::read_dir(source).map_err(|e| e.to_string())? {
            let e = e.map_err(|e| e.to_string())?;
            let kind = e.file_type().map_err(|e| e.to_string())?;
            let name = e.file_name();
            if kind.is_symlink() {
                return Err("symlink refused".into());
            }
            if name == "checkpoint-v2.json" || name == "runner.lock" {
                continue;
            }
            let dest = target.join(name);
            if kind.is_dir() {
                copy(&e.path(), &dest)?
            } else if kind.is_file() {
                fs::copy(e.path(), &dest).map_err(|e| e.to_string())?;
                if hash(&fs::read(e.path()).map_err(|e| e.to_string())?)
                    != hash(&fs::read(dest).map_err(|e| e.to_string())?)
                {
                    return Err("copy hash mismatch".into());
                }
            } else {
                return Err("unsupported file".into());
            }
        }
        Ok(())
    }
    if disk_used(source)? > new["budgets"]["jobDiskBytes"].as_u64().unwrap_or(u64::MAX) {
        return Err("source exceeds continuation disk cap".into());
    }
    copy(source, target)?;
    state["configSha256"] = json!(hash(&encode(new)?));
    state["startedAtUnixSeconds"] = json!(now);
    state["expiresAtUnixSeconds"] = json!(expiry);
    state["continuationParent"] = json!({"checkpointSha256":hash(&parent),"configSha256":hash(&encode(old)?),"chargedPages":state["chargedPages"],"chargedBytes":state["chargedBytes"]});
    if let Some(recovery) = recovery {
        for item in old["states"].as_array().unwrap() {
            let code = item["state"].as_str().unwrap();
            let path = recovery.join(format!("{code}-capture.json"));
            if !path.exists() {
                continue;
            }
            if state["states"][code]["status"] != "failed" {
                return Err("recovery requires failed reserved attempt".into());
            }
            let raw = read_capped(&path, 8 * MAX_BYTES)?;
            let capture: Value = serde_json::from_slice(&raw).map_err(|e| e.to_string())?;
            let pages = capture["acquisitions"].as_array().ok_or("missing pages")?;
            if pages.len() != 1 {
                return Err("recovery must be one reserved page".into());
            }
            let page = &pages[0];
            let mut verified = None;
            for e in
                fs::read_dir(source.join(format!("cache-{code}"))).map_err(|e| e.to_string())?
            {
                let e = e.map_err(|e| e.to_string())?;
                let cache: Value = serde_json::from_slice(&read_capped(&e.path(), 8 * MAX_BYTES)?)
                    .map_err(|e| e.to_string())?;
                if cache["url"] == page["url"] {
                    let body = cache["body"].as_str().ok_or("missing body")?;
                    if cache["sha256"] != hash(body.as_bytes()) {
                        return Err("recovery cache mismatch".into());
                    }
                    if verified.is_some() {
                        return Err("ambiguous cached recovery URL".into());
                    }
                    verified = Some(
                        json!({"schemaVersion":1,"acquisitions":[{"collection":"monitoring-locations","url":cache["url"],"retrievedAt":cache["retrievedAt"],"response":serde_json::from_str::<Value>(body).map_err(|e|e.to_string())?}]}),
                    );
                }
            }
            let capture = verified.ok_or("no immutable cached recovery response")?;
            let raw = encode(&capture)?;
            let bundle = gev_us_groundwater::inventory_bundle(&raw, code)?;
            let index = indices.get_mut(code).unwrap();
            let delta = examine(
                &capture,
                &bundle,
                code,
                new["budgets"]["rowsPerPage"].as_u64().unwrap(),
                index,
            )?;
            let capture_name = immutable(target, &format!("{code}-capture"), &raw)?;
            let bytes = encode(&bundle)?;
            let candidate = immutable(target, &format!("{code}-candidate"), &bytes)?;
            let part = json!({"capture":capture_name,"candidate":candidate,"candidateSha256":hash(&bytes),"rows":delta.ids.len(),"pages":delta.pages.len(),"nextPage":delta.next});
            apply(index, delta);
            let r = &mut state["states"][code];
            r["partitions"].as_array_mut().unwrap().push(part);
            r["accepted"] = json!(index.ids.len());
            r["quarantined"] = json!(index.quarantined.len());
            r["pages"] = json!(index.page_count);
            r["nextPage"] = json!(index.next);
            r["error"] = Value::Null;
            r["status"] = json!(if index.next.is_some() {
                "partial"
            } else {
                "completed"
            });
        }
    }
    if fs::read(source.join("checkpoint-v2.json")).map_err(|e| e.to_string())? != parent {
        return Err("parent changed".into());
    }
    if disk_used(target)? > new["budgets"]["jobDiskBytes"].as_u64().unwrap_or(u64::MAX) {
        return Err("migration disk cap".into());
    }
    persist(&target.join("checkpoint-v2.json"), &mut state)?;
    load(new, target).map(|(s, _)| s)
}

fn window(old: &Value, new: &Value, expiry: u64, now: u64) -> Result<(), String> {
    validate_config(old)?;
    validate_config(new)?;
    let mut expected = old.clone();
    if new.get("stopOnFailure").is_some() {
        if new["stopOnFailure"] != true {
            return Err("continuation stop policy may only tighten".into());
        }
        expected["stopOnFailure"] = json!(true);
    }
    // Disk is a continuation budget too: it may be raised explicitly, but a
    // continuation must never shrink the already-authorized artifact ceiling.
    if new["budgets"]["jobDiskBytes"].as_u64().unwrap_or(u64::MAX)
        < old["budgets"]["jobDiskBytes"].as_u64().unwrap_or(u64::MAX)
    {
        return Err("continuation disk cap may not decrease".into());
    }
    if old.get("absoluteExpiry").is_some() || new.get("absoluteExpiry").is_some() {
        if new["absoluteExpiry"].as_u64() != Some(expiry) {
            return Err("queue expiry must match explicit amendment expiry".into());
        }
        expected["absoluteExpiry"] = new["absoluteExpiry"].clone();
    }
    for key in [
        "jobPages",
        "jobBytes",
        "jobDiskBytes",
        "runPages",
        "runBytes",
        "runSeconds",
    ] {
        expected["budgets"][key] = new["budgets"][key].clone();
    }
    if expected != *new
        || expiry <= now
        || expiry - now > new["budgets"]["runSeconds"].as_u64().unwrap()
    {
        return Err("invalid continuation scope/window".into());
    }
    Ok(())
}

/// Amend only the checkpoint. Immutable config files are the CLI's active-config
/// references, so there is no second mutable file to commit alongside it.
pub(crate) fn amend(
    old: &Value,
    dir: &Path,
    new: &Value,
    expiry: u64,
    now: u64,
) -> Result<Value, String> {
    amend_inner(old, dir, new, expiry, now, false)
}
pub(super) fn amend_inner(
    old: &Value,
    dir: &Path,
    new: &Value,
    expiry: u64,
    now: u64,
    interrupt: bool,
) -> Result<Value, String> {
    window(old, new, expiry, now)?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .open(dir.join("runner.lock"))
        .map_err(|e| e.to_string())?;
    lock.try_lock().map_err(|_| "source job is active")?;
    let parent = fs::read(dir.join("checkpoint-v2.json")).map_err(|e| e.to_string())?;
    let (mut state, _) = load(old, dir)?;
    if state["networkBlocked"] == true || state["quotaBlocked"] == true {
        return Err("blocked source cannot be renewed".into());
    }
    if new["budgets"]["jobPages"].as_u64().unwrap() < state["chargedPages"].as_u64().unwrap()
        || new["budgets"]["jobBytes"].as_u64().unwrap() < state["chargedBytes"].as_u64().unwrap()
    {
        return Err("amendment caps below lifetime charges".into());
    }
    let evidence = dir.join("amendments");
    if evidence.exists()
        && fs::symlink_metadata(&evidence)
            .map_err(|e| e.to_string())?
            .file_type()
            .is_symlink()
    {
        return Err("linked evidence directory refused".into());
    }
    fs::create_dir_all(&evidence).map_err(|e| e.to_string())?;
    let parent_name = immutable(&evidence, "checkpoint", &parent)?;
    let old_name = immutable(&evidence, "config", &encode(old)?)?;
    let new_name = immutable(&evidence, "config", &encode(new)?)?;
    let record = json!({"schemaVersion":1,"parentCheckpoint":parent_name,"parentSha256":hash(&parent),"previousConfig":old_name,"config":new_name,"chargedPages":state["chargedPages"],"chargedBytes":state["chargedBytes"],"startedAtUnixSeconds":now,"expiresAtUnixSeconds":expiry});
    let record_name = immutable(&evidence, "amendment", &encode(&record)?)?;
    state["configSha256"] = json!(hash(&encode(new)?));
    state["startedAtUnixSeconds"] = json!(now);
    state["expiresAtUnixSeconds"] = json!(expiry);
    state["continuationParent"] = json!({"checkpointSha256":hash(&parent),"configSha256":hash(&encode(old)?),"chargedPages":state["chargedPages"],"chargedBytes":state["chargedBytes"]});
    state["amendment"] = json!({"record":format!("amendments/{record_name}"),"config":format!("amendments/{new_name}")});
    let limit = new["budgets"]["jobDiskBytes"].as_u64().unwrap_or(u64::MAX);
    if disk_used(dir)?.saturating_add(encode(&state)?.len() as u64 + 256) > limit {
        return Err("amendment disk cap".into());
    }
    if fs::read(dir.join("checkpoint-v2.json")).map_err(|e| e.to_string())? != parent {
        return Err("parent changed".into());
    }
    // A crash here leaves only unreferenced immutable evidence; old config and
    // checkpoint remain usable. The existing durable atomic rename commits last.
    if interrupt {
        return Err("simulated interruption before commit".into());
    }
    persist(&dir.join("checkpoint-v2.json"), &mut state)?;
    Ok(state)
}
