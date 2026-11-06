use crate::config::{self, Config};
use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;
use serde_json::Value;
use sha1::{Digest, Sha1};
use std::{
    collections::HashMap,
    fs::{self, File},
    io::{self, Read},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
};

const MANIFEST_URL: &str = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";
const ASSET_URL: &str = "https://resources.download.minecraft.net";
const LAUNCHER_NAME: &str = "nix-launcher";
const LAUNCHER_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Deserialize, Clone, Debug)]
pub struct VersionEntry {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub url: String,
}

#[derive(Deserialize)]
struct Latest {
    release: String,
}

#[derive(Deserialize)]
struct Manifest {
    latest: Latest,
    versions: Vec<VersionEntry>,
}

pub struct VersionList {
    pub latest_release: String,
    pub versions: Vec<VersionEntry>,
}

#[derive(Default, Clone)]
pub struct Progress {
    pub message: String,
    pub done: usize,
    pub total: usize,
    pub busy: bool,
    pub error: Option<String>,
}

pub type Shared = Arc<Mutex<Progress>>;

fn set(progress: &Shared, message: &str, done: usize, total: usize) {
    if let Ok(mut p) = progress.lock() {
        p.message = message.to_string();
        p.done = done;
        p.total = total;
    }
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .try_proxy_from_env(true)
        .user_agent(&format!("{LAUNCHER_NAME}/{LAUNCHER_VERSION}"))
        .timeout_connect(std::time::Duration::from_secs(15))
        .timeout_read(std::time::Duration::from_secs(60))
        .build()
}

fn get_json(url: &str) -> Result<Value> {
    let resp = agent()
        .get(url)
        .call()
        .with_context(|| format!("request failed: {url}"))?;
    Ok(serde_json::from_reader(resp.into_reader())?)
}

pub fn fetch_versions() -> Result<VersionList> {
    let manifest: Manifest = serde_json::from_value(get_json(MANIFEST_URL)?)?;
    Ok(VersionList {
        latest_release: manifest.latest.release,
        versions: manifest.versions,
    })
}

fn file_sha1(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha1::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

fn try_download(url: &str, dest: &Path, sha1: Option<&str>) -> Result<()> {
    let mut tmp = dest.as_os_str().to_owned();
    tmp.push(".part");
    let tmp = PathBuf::from(tmp);
    let resp = agent().get(url).call()?;
    let mut reader = resp.into_reader();
    {
        let mut file = File::create(&tmp)?;
        io::copy(&mut reader, &mut file)?;
    }
    if let Some(expected) = sha1 {
        let actual = file_sha1(&tmp)?;
        if !actual.eq_ignore_ascii_case(expected) {
            let _ = fs::remove_file(&tmp);
            bail!("checksum mismatch for {url}");
        }
    }
    fs::rename(&tmp, dest)?;
    Ok(())
}

fn download(url: &str, dest: &Path, sha1: Option<&str>, verify_existing: bool) -> Result<()> {
    if dest.exists() {
        match (sha1, verify_existing) {
            (Some(h), true) => {
                if file_sha1(dest)?.eq_ignore_ascii_case(h) {
                    return Ok(());
                }
            }
            _ => return Ok(()),
        }
    }
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut last = None;
    for _ in 0..3 {
        match try_download(url, dest, sha1) {
            Ok(()) => return Ok(()),
            Err(e) => last = Some(e),
        }
    }
    Err(last.unwrap_or_else(|| anyhow!("download failed")))
}

struct Job {
    url: String,
    dest: PathBuf,
    sha1: Option<String>,
    verify: bool,
}

fn run_jobs(jobs: Vec<Job>, progress: &Shared, label: &str) -> Result<()> {
    let total = jobs.len();
    if total == 0 {
        return Ok(());
    }
    set(progress, label, 0, total);
    let next = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    let failure: Mutex<Option<String>> = Mutex::new(None);
    std::thread::scope(|scope| {
        for _ in 0..8.min(total) {
            scope.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::SeqCst);
                if i >= total || failure.lock().map(|f| f.is_some()).unwrap_or(true) {
                    break;
                }
                let job = &jobs[i];
                if let Err(e) = download(&job.url, &job.dest, job.sha1.as_deref(), job.verify) {
                    if let Ok(mut f) = failure.lock() {
                        *f = Some(format!("{}: {e}", job.url));
                    }
                    break;
                }
                let d = done.fetch_add(1, Ordering::SeqCst) + 1;
                set(progress, label, d, total);
            });
        }
    });
    match failure.into_inner().ok().flatten() {
        Some(e) => Err(anyhow!(e)),
        None => Ok(()),
    }
}

fn rule_matches(rule: &Value) -> bool {
    if rule.get("features").is_some() {
        return false;
    }
    let Some(os) = rule.get("os") else {
        return true;
    };
    let arm = std::env::consts::ARCH == "aarch64";
    if let Some(name) = os.get("name").and_then(Value::as_str) {
        let ok = match name {
            "linux" => !arm,
            "linux-arm64" => arm,
            _ => false,
        };
        if !ok {
            return false;
        }
    }
    if let Some(arch) = os.get("arch").and_then(Value::as_str) {
        let ok = match arch {
            "x86" => false,
            "arm64" | "aarch64" => arm,
            _ => !arm,
        };
        if !ok {
            return false;
        }
    }
    true
}

fn rules_allow(rules: Option<&Value>) -> bool {
    let Some(list) = rules.and_then(Value::as_array) else {
        return true;
    };
    let mut allowed = false;
    for rule in list {
        if rule_matches(rule) {
            allowed = rule.get("action").and_then(Value::as_str) == Some("allow");
        }
    }
    allowed
}

fn substitute(template: &str, vars: &HashMap<&str, String>) -> String {
    let mut out = template.to_string();
    for (key, value) in vars {
        out = out.replace(&format!("${{{key}}}"), value);
    }
    out
}

fn collect_args(list: Option<&Value>, vars: &HashMap<&str, String>) -> Vec<String> {
    let mut out = Vec::new();
    for item in list.and_then(Value::as_array).into_iter().flatten() {
        match item {
            Value::String(s) => out.push(substitute(s, vars)),
            Value::Object(_) => {
                if !rules_allow(item.get("rules")) {
                    continue;
                }
                match &item["value"] {
                    Value::String(s) => out.push(substitute(s, vars)),
                    Value::Array(values) => {
                        for v in values.iter().filter_map(Value::as_str) {
                            out.push(substitute(v, vars));
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    out
}

pub fn offline_uuid(name: &str) -> String {
    let mut b = md5::compute(format!("OfflinePlayer:{name}")).0;
    b[6] = (b[6] & 0x0f) | 0x30;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

fn java_major(path: &Path) -> Option<u64> {
    let out = Command::new(path).arg("-version").output().ok()?;
    let text = String::from_utf8_lossy(&out.stderr).to_string();
    let line = text.lines().find(|l| l.contains(" version \""))?;
    let quoted = line.split('"').nth(1)?;
    let mut parts = quoted.split(['.', '_', '-', '+']);
    let first: u64 = parts.next()?.parse().ok()?;
    if first == 1 {
        parts.next()?.parse().ok()
    } else {
        Some(first)
    }
}

fn java_fits(found: u64, required: u64) -> bool {
    if required <= 8 {
        found == 8
    } else {
        found >= required
    }
}

fn managed_java(major: u64) -> PathBuf {
    config::data_dir()
        .join("runtimes")
        .join(format!("java-{major}"))
        .join("bin")
        .join("java")
}

fn adoptium_major(required: u64) -> u64 {
    if required == 16 {
        17
    } else {
        required
    }
}

fn find_java(required: u64, custom: &str) -> Option<PathBuf> {
    if !custom.trim().is_empty() {
        return Some(PathBuf::from(custom.trim()));
    }
    let mut candidates = vec![
        managed_java(adoptium_major(required)),
        PathBuf::from("java"),
    ];
    if let Ok(entries) = fs::read_dir("/usr/lib/jvm") {
        for entry in entries.flatten() {
            candidates.push(entry.path().join("bin").join("java"));
        }
    }
    candidates.into_iter().find(|c| {
        java_major(c)
            .map(|m| java_fits(m, required))
            .unwrap_or(false)
    })
}

fn install_java(required: u64, progress: &Shared) -> Result<PathBuf> {
    let major = adoptium_major(required);
    let root = config::data_dir().join("runtimes");
    let target = root.join(format!("java-{major}"));
    let arch = if std::env::consts::ARCH == "aarch64" {
        "aarch64"
    } else {
        "x64"
    };
    let url = format!(
        "https://api.adoptium.net/v3/binary/latest/{major}/ga/linux/{arch}/jre/hotspot/normal/eclipse"
    );
    fs::create_dir_all(&root)?;
    let archive = root.join(format!("java-{major}.tar.gz"));
    set(progress, &format!("Downloading Java {major}"), 0, 0);
    let _ = fs::remove_file(&archive);
    try_download(&url, &archive, None).context("could not download Java runtime")?;
    set(progress, &format!("Installing Java {major}"), 0, 0);
    let _ = fs::remove_dir_all(&target);
    fs::create_dir_all(&target)?;
    let result = (|| -> Result<()> {
        let file = File::open(&archive)?;
        let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(file));
        for entry in tar.entries()? {
            let mut entry = entry?;
            let path = entry.path()?.into_owned();
            let rel: PathBuf = path.components().skip(1).collect();
            if rel.as_os_str().is_empty() {
                continue;
            }
            let dest = target.join(&rel);
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent)?;
            }
            entry.unpack(&dest)?;
        }
        Ok(())
    })();
    let _ = fs::remove_file(&archive);
    if let Err(e) = result {
        let _ = fs::remove_dir_all(&target);
        return Err(e.context("could not unpack Java runtime"));
    }
    let bin = managed_java(major);
    if !bin.exists() {
        bail!("Java runtime archive did not contain bin/java");
    }
    Ok(bin)
}

fn extract_natives(jar: &Path, dest: &Path, excludes: &[String]) -> Result<()> {
    let mut archive = zip::ZipArchive::new(File::open(jar)?)?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let name = entry.name().to_string();
        if entry.is_dir() || excludes.iter().any(|e| name.starts_with(e.as_str())) {
            continue;
        }
        let Some(file_name) = Path::new(&name).file_name() else {
            continue;
        };
        fs::create_dir_all(dest)?;
        let mut out = File::create(dest.join(file_name))?;
        io::copy(&mut entry, &mut out)?;
    }
    Ok(())
}

pub fn run(cfg: &Config, entry: &VersionEntry, progress: &Shared) -> Result<()> {
    let root = config::data_dir();
    let version_dir = root.join("versions").join(&entry.id);
    let libraries_dir = root.join("libraries");
    let assets_dir = root.join("assets");
    let game_dir = root.join("game");
    fs::create_dir_all(&version_dir)?;
    fs::create_dir_all(&game_dir)?;

    set(progress, "Fetching version data", 0, 0);
    let json_path = version_dir.join(format!("{}.json", entry.id));
    let vjson: Value = match fs::read(&json_path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
    {
        Some(v) => v,
        None => {
            let v = get_json(&entry.url)?;
            fs::write(&json_path, serde_json::to_vec(&v)?)?;
            v
        }
    };
    if vjson.get("inheritsFrom").is_some() {
        bail!("modded version profiles are not supported yet");
    }

    let mut jobs = Vec::new();
    let client = &vjson["downloads"]["client"];
    let client_jar = version_dir.join(format!("{}.jar", entry.id));
    jobs.push(Job {
        url: client["url"]
            .as_str()
            .context("missing client download")?
            .to_string(),
        dest: client_jar.clone(),
        sha1: client["sha1"].as_str().map(str::to_string),
        verify: false,
    });

    let mut classpath: Vec<PathBuf> = Vec::new();
    let mut natives: Vec<(PathBuf, Vec<String>)> = Vec::new();
    for lib in vjson["libraries"].as_array().into_iter().flatten() {
        if !rules_allow(lib.get("rules")) {
            continue;
        }
        let downloads = &lib["downloads"];
        if let Some(artifact) = downloads.get("artifact").filter(|a| a.get("url").is_some()) {
            let path = libraries_dir.join(artifact["path"].as_str().unwrap_or_default());
            jobs.push(Job {
                url: artifact["url"].as_str().unwrap_or_default().to_string(),
                dest: path.clone(),
                sha1: artifact["sha1"].as_str().map(str::to_string),
                verify: true,
            });
            classpath.push(path);
        }
        let classifier = lib["natives"]["linux"]
            .as_str()
            .map(|c| c.replace("${arch}", "64"));
        if let Some(classifier) = classifier {
            if let Some(native) = downloads["classifiers"].get(&classifier) {
                let path = libraries_dir.join(native["path"].as_str().unwrap_or_default());
                jobs.push(Job {
                    url: native["url"].as_str().unwrap_or_default().to_string(),
                    dest: path.clone(),
                    sha1: native["sha1"].as_str().map(str::to_string),
                    verify: true,
                });
                let excludes = lib["extract"]["exclude"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default();
                natives.push((path, excludes));
            }
        }
    }
    classpath.push(client_jar);

    let index = &vjson["assetIndex"];
    let index_id = index["id"]
        .as_str()
        .context("missing asset index")?
        .to_string();
    let index_path = assets_dir.join("indexes").join(format!("{index_id}.json"));
    set(progress, "Fetching asset index", 0, 0);
    download(
        index["url"].as_str().unwrap_or_default(),
        &index_path,
        index["sha1"].as_str(),
        true,
    )?;
    let index_json: Value = serde_json::from_slice(&fs::read(&index_path)?)?;
    let objects = index_json["objects"]
        .as_object()
        .cloned()
        .unwrap_or_default();
    let mut asset_jobs = Vec::new();
    for (_, object) in objects.iter() {
        let hash = object["hash"].as_str().unwrap_or_default();
        if hash.len() < 2 {
            continue;
        }
        asset_jobs.push(Job {
            url: format!("{ASSET_URL}/{}/{hash}", &hash[..2]),
            dest: assets_dir.join("objects").join(&hash[..2]).join(hash),
            sha1: Some(hash.to_string()),
            verify: false,
        });
    }

    run_jobs(jobs, progress, "Downloading game files")?;
    run_jobs(asset_jobs, progress, "Downloading assets")?;

    let legacy_assets = index_json["map_to_resources"].as_bool().unwrap_or(false)
        || index_json["virtual"].as_bool().unwrap_or(false);
    let mut game_assets = assets_dir.join("virtual").join("legacy");
    if legacy_assets {
        set(progress, "Preparing legacy assets", 0, 0);
        for (name, object) in objects.iter() {
            let hash = object["hash"].as_str().unwrap_or_default();
            if hash.len() < 2 {
                continue;
            }
            let source = assets_dir.join("objects").join(&hash[..2]).join(hash);
            let target = game_assets.join(name);
            if !target.exists() {
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::copy(&source, &target)?;
            }
        }
        if index_json["map_to_resources"].as_bool().unwrap_or(false) {
            game_assets = game_dir.join("resources");
            for (name, object) in objects.iter() {
                let hash = object["hash"].as_str().unwrap_or_default();
                if hash.len() < 2 {
                    continue;
                }
                let source = assets_dir.join("objects").join(&hash[..2]).join(hash);
                let target = game_assets.join(name);
                if !target.exists() {
                    if let Some(parent) = target.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    fs::copy(&source, &target)?;
                }
            }
        }
    } else {
        game_assets = assets_dir.clone();
    }

    let natives_dir = version_dir.join("natives");
    if !natives.is_empty() {
        set(progress, "Extracting natives", 0, 0);
        let _ = fs::remove_dir_all(&natives_dir);
        for (jar, excludes) in &natives {
            extract_natives(jar, &natives_dir, excludes)?;
        }
    }
    fs::create_dir_all(&natives_dir)?;

    let required = vjson["javaVersion"]["majorVersion"].as_u64().unwrap_or(8);
    let java = match find_java(required, &cfg.java_path) {
        Some(j) => j,
        None => install_java(required, progress)?,
    };

    let separator = ":";
    let classpath_str = classpath
        .iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect::<Vec<_>>()
        .join(separator);

    let vars: HashMap<&str, String> = HashMap::from([
        ("auth_player_name", cfg.nickname.clone()),
        ("version_name", entry.id.clone()),
        ("game_directory", game_dir.to_string_lossy().to_string()),
        ("assets_root", assets_dir.to_string_lossy().to_string()),
        ("game_assets", game_assets.to_string_lossy().to_string()),
        ("assets_index_name", index_id.clone()),
        ("auth_uuid", offline_uuid(&cfg.nickname)),
        ("auth_access_token", "0".to_string()),
        ("auth_session", "0".to_string()),
        ("user_type", "legacy".to_string()),
        ("user_properties", "{}".to_string()),
        ("version_type", entry.kind.clone()),
        ("clientid", "0".to_string()),
        ("auth_xuid", "0".to_string()),
        (
            "natives_directory",
            natives_dir.to_string_lossy().to_string(),
        ),
        ("launcher_name", LAUNCHER_NAME.to_string()),
        ("launcher_version", LAUNCHER_VERSION.to_string()),
        ("classpath", classpath_str.clone()),
        (
            "library_directory",
            libraries_dir.to_string_lossy().to_string(),
        ),
        ("classpath_separator", separator.to_string()),
    ]);

    let mut args: Vec<String> = vec![format!("-Xmx{}M", cfg.ram_mb), "-Xms512M".to_string()];
    if vjson["arguments"]["jvm"].is_array() {
        args.extend(collect_args(Some(&vjson["arguments"]["jvm"]), &vars));
    } else {
        args.push(format!("-Djava.library.path={}", natives_dir.display()));
        args.push("-cp".to_string());
        args.push(classpath_str);
    }
    args.push(
        vjson["mainClass"]
            .as_str()
            .context("missing main class")?
            .to_string(),
    );
    if vjson["arguments"]["game"].is_array() {
        args.extend(collect_args(Some(&vjson["arguments"]["game"]), &vars));
    } else if let Some(legacy) = vjson["minecraftArguments"].as_str() {
        args.extend(legacy.split_whitespace().map(|a| substitute(a, &vars)));
    }

    let log_dir = root.join("logs");
    fs::create_dir_all(&log_dir)?;
    let log = File::create(log_dir.join("latest.log"))?;
    let log_err = log.try_clone()?;

    set(progress, &format!("Playing Minecraft {}", entry.id), 0, 0);
    let mut child = Command::new(&java)
        .args(&args)
        .current_dir(&game_dir)
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log_err))
        .spawn()
        .with_context(|| format!("could not start {}", java.display()))?;
    let status = child.wait()?;
    if !status.success() {
        bail!(
            "the game exited with {status}. See {}",
            log_dir.join("latest.log").display()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn offline_uuid_matches_known_value() {
        assert_eq!(
            offline_uuid("Notch"),
            "b50ad385-829d-3141-a216-7e7d7539ba7f"
        );
    }

    #[test]
    fn rules_respect_os_and_features() {
        let linux = json!([{"action": "allow", "os": {"name": "linux"}}]);
        let windows = json!([{"action": "allow", "os": {"name": "windows"}}]);
        let feature = json!([{"action": "allow", "features": {"is_demo_user": true}}]);
        let denied = json!([{"action": "allow"}, {"action": "disallow", "os": {"name": "linux"}}]);
        assert!(rules_allow(Some(&linux)) == (std::env::consts::ARCH != "aarch64"));
        assert!(!rules_allow(Some(&windows)));
        assert!(!rules_allow(Some(&feature)));
        assert!(!rules_allow(Some(&denied)));
        assert!(rules_allow(None));
    }

    #[test]
    fn arguments_are_substituted() {
        let vars = HashMap::from([("auth_player_name", "Steve".to_string())]);
        let list = json!(["--username", "${auth_player_name}", {"rules": [{"action": "allow", "os": {"name": "windows"}}], "value": "-Dwin"}, {"rules": [{"action": "allow", "os": {"name": "linux"}}], "value": ["-Da", "-Db"]}]);
        let out = collect_args(Some(&list), &vars);
        assert_eq!(&out[..2], ["--username", "Steve"]);
        assert!(!out.contains(&"-Dwin".to_string()));
    }

    #[test]
    fn java_version_matching() {
        assert!(java_fits(8, 8));
        assert!(!java_fits(17, 8));
        assert!(java_fits(21, 17));
        assert!(!java_fits(11, 17));
    }
}
