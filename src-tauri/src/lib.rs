use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use futures::StreamExt;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;
use sha1::Sha1;
use sha2::{Digest, Sha256, Sha512};
#[cfg(target_os = "linux")]
use std::os::unix::fs::MetadataExt;
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    env,
    fs::{self, OpenOptions},
    io::{BufRead, BufReader, Cursor, Read, Seek, SeekFrom, Write},
    net::{SocketAddr, TcpStream},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex, OnceLock,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tauri::Emitter;
use tauri::Manager;
#[cfg(windows)]
use windows_sys::Win32::{
    Foundation::LocalFree,
    Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    },
};

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;
const MICROSOFT_CLIENT_ID: &str = "00000000402b5328";
const MICROSOFT_REDIRECT_URI: &str = "https://login.live.com/oauth20_desktop.srf";
const VEX_USER_AGENT: &str = concat!(
    "ExtremestoneGG/VEX-Launcher/",
    env!("CARGO_PKG_VERSION"),
    " (https://github.com/ExtremestoneGG/VEX-Launcher)"
);

#[cfg(not(test))]
fn storage_pointer_path() -> PathBuf {
    #[cfg(target_os = "linux")]
    {
        env::var("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|_| env::var("HOME").map(|home| PathBuf::from(home).join(".config")))
            .unwrap_or_else(|_| env::temp_dir())
            .join("vex-launcher")
            .join("storage-root")
    }
    #[cfg(windows)]
    {
        env::var("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| env::temp_dir())
            .join("VEX Launcher")
            .join("storage-root")
    }
    #[cfg(all(not(target_os = "linux"), not(windows)))]
    {
        env::var("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| env::temp_dir())
            .join(".config")
            .join("vex-launcher")
            .join("storage-root")
    }
}

#[cfg(not(test))]
fn stored_storage_root() -> Option<PathBuf> {
    let content = fs::read_to_string(storage_pointer_path()).ok()?;
    let clean = content.trim();
    if clean.is_empty() || clean.len() > 4096 {
        return None;
    }
    let root = PathBuf::from(clean);
    root.is_absolute().then_some(root)
}

fn configured_storage_root() -> Option<PathBuf> {
    let from_environment = env::var_os("VEX_STORAGE_ROOT")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    #[cfg(test)]
    {
        from_environment
    }
    #[cfg(not(test))]
    {
        from_environment.or_else(stored_storage_root)
    }
}

#[cfg(not(test))]
fn persist_storage_root_pointer(root: &Path) -> Result<(), String> {
    if !root.is_absolute() {
        return Err(String::from(
            "VEX_STORAGE_ROOT precisa ser um caminho absoluto.",
        ));
    }
    let pointer = storage_pointer_path();
    fs::create_dir_all(
        pointer
            .parent()
            .ok_or_else(|| String::from("Caminho de configuração do VEX inválido."))?,
    )
    .map_err(|error| error.to_string())?;
    fs::write(pointer, format!("{}\n", root.to_string_lossy())).map_err(|error| error.to_string())
}

#[cfg(not(test))]
fn persist_environment_storage_root() -> Result<(), String> {
    let Some(root) = env::var_os("VEX_STORAGE_ROOT")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
    else {
        return Ok(());
    };
    persist_storage_root_pointer(&root)
}

#[cfg(not(test))]
fn storage_root_is_configured() -> bool {
    env::var_os("VEX_STORAGE_ROOT").is_some() || stored_storage_root().is_some()
}

#[cfg(test)]
fn storage_root_is_configured() -> bool {
    configured_storage_root().is_some()
}

#[cfg(test)]
fn persist_storage_root_pointer(_root: &Path) -> Result<(), String> {
    Ok(())
}

fn ensure_storage_layout(root: &Path) -> Result<(), String> {
    for folder in [
        "cache",
        "downloads",
        "exports",
        "instances",
        "logs",
        "maps",
        "profiles",
        "runtimes",
        "servers",
        "temp",
    ] {
        fs::create_dir_all(root.join(folder)).map_err(|error| {
            format!(
                "Não foi possível preparar a pasta {} do VEX: {error}",
                root.join(folder).display()
            )
        })?;
    }
    Ok(())
}

#[cfg(test)]
fn storage_root() -> PathBuf {
    configured_storage_root().unwrap_or_else(|| env::temp_dir().join("vex-launcher-tests"))
}

#[cfg(not(test))]
fn storage_root() -> PathBuf {
    if let Some(root) = configured_storage_root() {
        return root;
    }
    #[cfg(not(windows))]
    {
        env::var("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|_| {
                env::var("HOME").map(|home| PathBuf::from(home).join(".local").join("share"))
            })
            .unwrap_or_else(|_| env::temp_dir())
            .join("vex-launcher")
    }
    #[cfg(windows)]
    env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| env::temp_dir())
        .join("VEX Launcher")
}
fn default_game_directory() -> PathBuf {
    storage_root().join(".minecraft")
}

#[derive(Clone, Serialize)]
struct OperationProgress {
    operation: String,
    label: String,
    percent: u8,
    done: bool,
}

fn emit_progress(
    app: &tauri::AppHandle,
    operation: &str,
    label: impl Into<String>,
    percent: u8,
    done: bool,
) {
    let _ = app.emit(
        "operation-progress",
        OperationProgress {
            operation: operation.to_owned(),
            label: label.into(),
            percent: percent.min(100),
            done,
        },
    );
}

fn hidden_command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    #[cfg(windows)]
    {
        let mut command = Command::new(program);
        command.creation_flags(CREATE_NO_WINDOW);
        command
    }
    #[cfg(not(windows))]
    {
        Command::new(program)
    }
}

#[derive(Serialize)]
struct StorageStatus {
    root: String,
    strategy: String,
    configured: bool,
    available_gb: f64,
    folders: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
struct StorageDisk {
    id: String,
    label: String,
    mount_path: String,
    root_path: String,
    available_gb: f64,
    total_gb: f64,
    removable: bool,
    current: bool,
    recommended: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct LauncherSettings {
    storage_root: String,
    game_directory: String,
    offline_username: String,
    offline_skin_path: Option<String>,
    use_offline_profile: bool,
    onboarding_completed: bool,
    mangohud_enabled: bool,
    minimize_on_launch: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredOfflineSkin {
    id: String,
    name: String,
    path: String,
    model: String,
    created_unix: u64,
}

#[derive(Debug, Clone, Serialize)]
struct OfflineSkinSummary {
    id: String,
    name: String,
    path: String,
    model: String,
    created_unix: u64,
    data_url: String,
    active: bool,
}

impl Default for LauncherSettings {
    fn default() -> Self {
        Self {
            storage_root: storage_root().to_string_lossy().into_owned(),
            game_directory: default_game_directory().to_string_lossy().into_owned(),
            offline_username: String::from("Player"),
            offline_skin_path: None,
            use_offline_profile: true,
            onboarding_completed: false,
            mangohud_enabled: false,
            minimize_on_launch: true,
        }
    }
}

fn settings_path() -> PathBuf {
    storage_root().join("settings.json")
}

fn read_settings() -> LauncherSettings {
    if !settings_path().exists() {
        let migrated = migrate_legacy_settings().unwrap_or_default();
        let _ = write_settings(&migrated);
        return migrated;
    }

    fs::read_to_string(settings_path())
        .ok()
        .and_then(|content| serde_json::from_str(&content).ok())
        .unwrap_or_default()
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct LegacySettings {
    game_directory: Option<String>,
    offline_skin_path: Option<String>,
    offline_username: Option<String>,
    use_offline_profile: Option<bool>,
}

fn migrate_legacy_settings() -> Option<LauncherSettings> {
    let appdata = env::var("APPDATA").ok()?;
    let legacy_path = PathBuf::from(appdata)
        .join("MinecraftLauncher")
        .join("settings.json");
    let legacy: LegacySettings =
        serde_json::from_str(&fs::read_to_string(legacy_path).ok()?).ok()?;
    let mut settings = LauncherSettings::default();
    if let Some(game_directory) = legacy
        .game_directory
        .filter(|path| Path::new(path).exists())
    {
        settings.game_directory = game_directory;
    }
    if let Some(username) = legacy
        .offline_username
        .filter(|name| !name.trim().is_empty())
    {
        settings.offline_username = username;
    }
    settings.use_offline_profile = legacy.use_offline_profile.unwrap_or(true);

    if let Some(source) = legacy
        .offline_skin_path
        .filter(|path| Path::new(path).is_file())
    {
        let destination = storage_root().join("profiles").join("offline_skin.png");
        if let Some(parent) = destination.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if fs::copy(source, &destination).is_ok() {
            settings.offline_skin_path = Some(destination.to_string_lossy().into_owned());
        }
    }
    Some(settings)
}

fn write_settings(settings: &LauncherSettings) -> Result<(), String> {
    let path = settings_path();
    let parent = path
        .parent()
        .ok_or_else(|| String::from("Invalid settings path"))?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let json = serde_json::to_string_pretty(settings).map_err(|error| error.to_string())?;
    fs::write(path, json).map_err(|error| error.to_string())
}

#[derive(Debug, Clone, Serialize, Default)]
struct MicrosoftAccountStatus {
    logged_in: bool,
    active: bool,
    username: String,
    uuid: String,
    skin_url: Option<String>,
    skin_data_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredMicrosoftAccount {
    username: String,
    uuid: String,
    skin_url: Option<String>,
    protected_refresh_token: String,
}

struct AuthenticatedMicrosoftAccount {
    username: String,
    uuid: String,
    skin_url: Option<String>,
    access_token: String,
    refresh_token: String,
}

fn microsoft_account_path() -> PathBuf {
    storage_root()
        .join("profiles")
        .join("microsoft-account.json")
}

static MICROSOFT_AUTH_LOG_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn microsoft_auth_log_path() -> PathBuf {
    storage_root().join("logs").join("microsoft-auth.log")
}

fn record_microsoft_auth_diagnostic(message: &str) {
    let _guard = MICROSOFT_AUTH_LOG_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let path = microsoft_auth_log_path();
    let Some(parent) = path.parent() else {
        return;
    };
    if fs::create_dir_all(parent).is_err() {
        return;
    }
    if fs::metadata(&path)
        .map(|metadata| metadata.len() > 512 * 1024)
        .unwrap_or(false)
    {
        let _ = fs::write(&path, "[VEX] Log de autenticação compactado.\n");
    }
    let clean = message
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>();
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "[{timestamp}] {clean}");
    }
}

fn microsoft_skin_path() -> PathBuf {
    storage_root().join("profiles").join("microsoft-skin.png")
}

fn offline_skin_library_dir() -> PathBuf {
    storage_root().join("profiles").join("skins")
}

fn offline_skin_library_path() -> PathBuf {
    offline_skin_library_dir().join("library.json")
}

fn write_offline_skin_library(skins: &[StoredOfflineSkin]) -> Result<(), String> {
    let path = offline_skin_library_path();
    fs::create_dir_all(
        path.parent()
            .ok_or_else(|| String::from("Caminho da biblioteca de skins inválido."))?,
    )
    .map_err(|error| error.to_string())?;
    fs::write(
        path,
        serde_json::to_string_pretty(skins).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())
}

fn read_offline_skin_library() -> Vec<StoredOfflineSkin> {
    let mut skins: Vec<StoredOfflineSkin> = fs::read_to_string(offline_skin_library_path())
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default();
    skins.retain(|skin| Path::new(&skin.path).is_file());

    if let Some(active_path) = read_settings()
        .offline_skin_path
        .filter(|path| Path::new(path).is_file())
    {
        if !skins.iter().any(|skin| skin.path == active_path) {
            let id = Sha256::digest(active_path.as_bytes())[..12]
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            skins.push(StoredOfflineSkin {
                id,
                name: String::from("Skin anterior"),
                path: active_path,
                model: String::from("auto-detect"),
                created_unix: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|duration| duration.as_secs())
                    .unwrap_or_default(),
            });
        }
    }
    let _ = write_offline_skin_library(&skins);
    skins
}

fn skin_summary(skin: &StoredOfflineSkin, active_path: Option<&str>) -> OfflineSkinSummary {
    OfflineSkinSummary {
        id: skin.id.clone(),
        name: skin.name.clone(),
        path: skin.path.clone(),
        model: skin.model.clone(),
        created_unix: skin.created_unix,
        data_url: image_data_url(Path::new(&skin.path), "image/png").unwrap_or_default(),
        active: active_path == Some(skin.path.as_str()),
    }
}

fn list_skin_summaries() -> Vec<OfflineSkinSummary> {
    let active_path = read_settings().offline_skin_path;
    let mut skins = read_offline_skin_library();
    skins.sort_by_key(|skin| std::cmp::Reverse(skin.created_unix));
    skins
        .iter()
        .map(|skin| skin_summary(skin, active_path.as_deref()))
        .collect()
}

fn normalized_minecraft_texture_url(value: &str) -> Option<String> {
    let mut url = reqwest::Url::parse(value).ok()?;
    if url.host_str() != Some("textures.minecraft.net") || !url.path().starts_with("/texture/") {
        return None;
    }
    url.set_scheme("https").ok()?;
    Some(url.to_string())
}

fn image_data_url(path: &Path, mime: &str) -> Option<String> {
    fs::read(path)
        .ok()
        .map(|bytes| format!("data:{mime};base64,{}", BASE64_STANDARD.encode(bytes)))
}

async fn cache_microsoft_skin(
    client: &reqwest::Client,
    skin_url: Option<&str>,
) -> Result<(), String> {
    let Some(url) = skin_url.and_then(normalized_minecraft_texture_url) else {
        return Ok(());
    };
    let bytes = client
        .get(&url)
        .send()
        .await
        .map_err(|error| error.to_string())?
        .error_for_status()
        .map_err(|error| error.to_string())?
        .bytes()
        .await
        .map_err(|error| error.to_string())?;
    if bytes.len() > 10 * 1024 * 1024 || !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err(String::from(
            "A skin Microsoft recebida não é um PNG válido.",
        ));
    }
    let path = microsoft_skin_path();
    fs::create_dir_all(
        path.parent()
            .ok_or_else(|| String::from("Caminho de skin Microsoft inválido."))?,
    )
    .map_err(|error| error.to_string())?;
    fs::write(path, bytes).map_err(|error| error.to_string())
}

#[cfg(windows)]
fn protect_secret(secret: &str) -> Result<String, String> {
    let bytes = secret.as_bytes();
    let input = CRYPT_INTEGER_BLOB {
        cbData: bytes
            .len()
            .try_into()
            .map_err(|_| String::from("Token Microsoft muito grande."))?,
        pbData: bytes.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    let ok = unsafe {
        CryptProtectData(
            &input,
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    };
    if ok == 0 {
        return Err(format!(
            "Não foi possível proteger a sessão Microsoft: {}",
            std::io::Error::last_os_error()
        ));
    }
    let protected =
        unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize) }.to_vec();
    unsafe {
        LocalFree(output.pbData as *mut std::ffi::c_void);
    }
    Ok(BASE64_STANDARD.encode(protected))
}

#[cfg(target_os = "linux")]
fn protect_secret(secret: &str) -> Result<String, String> {
    let entry = keyring::Entry::new("app.vexlauncher.desktop", "microsoft-refresh-token")
        .map_err(|error| format!("Não foi possível acessar o cofre de senhas do Linux: {error}"))?;
    entry.set_password(secret).map_err(|error| {
        format!("Não foi possível proteger a sessão Microsoft no Linux: {error}")
    })?;
    Ok(String::from("keyring:microsoft-refresh-token"))
}

#[cfg(all(not(windows), not(target_os = "linux")))]
fn protect_secret(_secret: &str) -> Result<String, String> {
    Err(String::from(
        "O armazenamento seguro da conta Microsoft não está disponível neste sistema.",
    ))
}

#[cfg(windows)]
fn unprotect_secret(protected: &str) -> Result<String, String> {
    let bytes = BASE64_STANDARD
        .decode(protected)
        .map_err(|_| String::from("Sessão Microsoft protegida inválida."))?;
    let input = CRYPT_INTEGER_BLOB {
        cbData: bytes
            .len()
            .try_into()
            .map_err(|_| String::from("Sessão Microsoft inválida."))?,
        pbData: bytes.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    let ok = unsafe {
        CryptUnprotectData(
            &input,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    };
    if ok == 0 {
        return Err(format!(
            "Não foi possível abrir a sessão Microsoft neste usuário do Windows: {}",
            std::io::Error::last_os_error()
        ));
    }
    let secret =
        unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize) }.to_vec();
    unsafe {
        LocalFree(output.pbData as *mut std::ffi::c_void);
    }
    String::from_utf8(secret).map_err(|_| String::from("Sessão Microsoft inválida."))
}

#[cfg(target_os = "linux")]
fn unprotect_secret(protected: &str) -> Result<String, String> {
    if protected != "keyring:microsoft-refresh-token" {
        return Err(String::from(
            "A sessão Microsoft salva usa um formato antigo. Entre novamente para migrá-la com segurança.",
        ));
    }
    keyring::Entry::new("app.vexlauncher.desktop", "microsoft-refresh-token")
        .map_err(|error| format!("Não foi possível acessar o cofre de senhas do Linux: {error}"))?
        .get_password()
        .map_err(|error| format!("Não foi possível abrir a sessão Microsoft no Linux: {error}"))
}

#[cfg(all(not(windows), not(target_os = "linux")))]
fn unprotect_secret(_protected: &str) -> Result<String, String> {
    Err(String::from(
        "O armazenamento seguro da conta Microsoft não está disponível neste sistema.",
    ))
}

#[cfg(target_os = "linux")]
fn delete_linux_microsoft_secret() -> Result<(), String> {
    let entry = keyring::Entry::new("app.vexlauncher.desktop", "microsoft-refresh-token")
        .map_err(|error| format!("Não foi possível acessar o cofre de senhas do Linux: {error}"))?;
    match entry.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => Err(format!(
            "Não foi possível remover a sessão Microsoft do Linux: {error}"
        )),
    }
}

fn curseforge_key_path() -> PathBuf {
    storage_root()
        .join("secrets")
        .join("curseforge-api-key.dat")
}

fn read_curseforge_api_key() -> Result<String, String> {
    if let Ok(key) = env::var("CURSEFORGE_API_KEY") {
        let key = key.trim().to_owned();
        if !key.is_empty() {
            return Ok(key);
        }
    }
    let stored = fs::read_to_string(curseforge_key_path())
        .map_err(|_| String::from("Configure a chave do CurseForge em Rede e fontes."))?;
    #[cfg(windows)]
    let key = unprotect_secret(stored.trim())?;
    #[cfg(not(windows))]
    let key = String::from_utf8(
        BASE64_STANDARD
            .decode(stored.trim())
            .map_err(|_| String::from("Chave do CurseForge inválida."))?,
    )
    .map_err(|_| String::from("Chave do CurseForge inválida."))?;
    if key.trim().is_empty() {
        return Err(String::from(
            "Configure a chave do CurseForge em Rede e fontes.",
        ));
    }
    Ok(key.trim().to_owned())
}

fn write_curseforge_api_key(key: &str) -> Result<(), String> {
    let clean = key.trim();
    if clean.len() < 16 || clean.chars().any(char::is_whitespace) {
        return Err(String::from(
            "A chave informada não parece ser uma chave válida do CurseForge.",
        ));
    }
    let path = curseforge_key_path();
    fs::create_dir_all(
        path.parent()
            .ok_or_else(|| String::from("Caminho de segredos inválido."))?,
    )
    .map_err(|error| error.to_string())?;
    #[cfg(windows)]
    let protected = protect_secret(clean)?;
    #[cfg(not(windows))]
    let protected = BASE64_STANDARD.encode(clean.as_bytes());
    fs::write(&path, protected).map_err(|error| error.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn curseforge_client() -> Result<reqwest::Client, String> {
    let key = read_curseforge_api_key()?;
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        "x-api-key",
        reqwest::header::HeaderValue::from_str(&key)
            .map_err(|_| String::from("Chave do CurseForge inválida."))?,
    );
    reqwest::Client::builder()
        .user_agent(VEX_USER_AGENT)
        .default_headers(headers)
        .build()
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn get_curseforge_status() -> CurseForgeStatus {
    if env::var("CURSEFORGE_API_KEY")
        .ok()
        .is_some_and(|key| !key.trim().is_empty())
    {
        return CurseForgeStatus {
            configured: true,
            source: String::from("Ambiente"),
        };
    }
    CurseForgeStatus {
        configured: read_curseforge_api_key().is_ok(),
        source: if curseforge_key_path().is_file() {
            String::from("Protegida no dispositivo")
        } else {
            String::from("Não configurada")
        },
    }
}

#[tauri::command]
fn save_curseforge_api_key(key: String) -> Result<CurseForgeStatus, String> {
    write_curseforge_api_key(&key)?;
    Ok(get_curseforge_status())
}

#[tauri::command]
fn remove_curseforge_api_key() -> Result<CurseForgeStatus, String> {
    let path = curseforge_key_path();
    if path.is_file() {
        fs::remove_file(path).map_err(|error| error.to_string())?;
    }
    Ok(get_curseforge_status())
}

fn read_microsoft_account() -> Option<StoredMicrosoftAccount> {
    fs::read_to_string(microsoft_account_path())
        .ok()
        .and_then(|content| serde_json::from_str(&content).ok())
}

fn write_microsoft_account(account: &AuthenticatedMicrosoftAccount) -> Result<(), String> {
    let path = microsoft_account_path();
    fs::create_dir_all(
        path.parent()
            .ok_or_else(|| String::from("Caminho de perfil Microsoft inválido."))?,
    )
    .map_err(|error| error.to_string())?;
    let stored = StoredMicrosoftAccount {
        username: account.username.clone(),
        uuid: account.uuid.clone(),
        skin_url: account.skin_url.clone(),
        protected_refresh_token: protect_secret(&account.refresh_token)?,
    };
    fs::write(
        path,
        serde_json::to_string_pretty(&stored).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())
}

fn microsoft_account_status_value() -> MicrosoftAccountStatus {
    let settings = read_settings();
    read_microsoft_account()
        .map(|account| MicrosoftAccountStatus {
            logged_in: true,
            active: !settings.use_offline_profile,
            username: account.username,
            uuid: account.uuid,
            skin_url: account
                .skin_url
                .as_deref()
                .and_then(normalized_minecraft_texture_url),
            skin_data_url: image_data_url(&microsoft_skin_path(), "image/png"),
        })
        .unwrap_or_default()
}

#[derive(Debug, Clone, Serialize)]
struct JavaRuntime {
    path: String,
    major: u32,
}

#[derive(Debug, Serialize)]
struct InstalledInstance {
    id: String,
    name: String,
    loader: String,
    mc_version: String,
    version_id: String,
    profile_dir: String,
    icon_path: Option<String>,
    kind: String,
    size_mb: f64,
    modified_unix: u64,
    last_played_unix: u64,
}

#[derive(Debug, Serialize)]
struct ModpackUpdateStatus {
    supported: bool,
    update_available: bool,
    project_id: String,
    project_name: String,
    author: String,
    game_version: String,
    current_version: String,
    latest_version: String,
    latest_version_id: String,
    message: String,
}

#[derive(Debug, Serialize)]
struct MinecraftInstallationScan {
    path: String,
    label: String,
    versions: Vec<String>,
    loaders: Vec<String>,
    mods: usize,
    worlds: usize,
    resource_packs: usize,
    shaders: usize,
    size_mb: f64,
    managed_by_vex: bool,
}

#[derive(Debug, Serialize)]
struct LoaderGameVersion {
    game_version: String,
    loader_version: String,
    recommended: bool,
}

#[derive(Debug, Serialize)]
struct LaunchResult {
    pid: u32,
    version_id: String,
    profile_dir: String,
    log_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ModrinthInstallTarget {
    instance_name: String,
    game_version: String,
    loader: String,
    destination_dir: String,
    download_url: String,
    filename: String,
    sha512: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CurseForgeInstallTarget {
    instance_name: String,
    game_version: String,
    loader: String,
    destination_dir: String,
    download_url: String,
    filename: String,
    md5: Option<String>,
}

#[derive(Debug, Serialize)]
struct CurseForgeStatus {
    configured: bool,
    source: String,
}

#[derive(Debug, Serialize)]
struct CurseForgeSearchResult {
    projects: Vec<CurseForgeProject>,
    total: u64,
}

#[derive(Debug, Serialize)]
struct CurseForgeProject {
    id: String,
    name: String,
    author: String,
    kind: String,
    description: String,
    versions: Vec<String>,
    downloads: u64,
    icon_url: Option<String>,
    page_url: Option<String>,
}

#[derive(Debug, Serialize)]
struct InstanceContent {
    name: String,
    path: String,
    kind: String,
    size_mb: f64,
    modified_unix: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct ServerProfile {
    id: String,
    name: String,
    version: String,
    software: String,
    memory_gb: u32,
    port: u16,
    max_players: u32,
    motd: String,
    online_mode: bool,
    gamemode: String,
    difficulty: String,
    directory: String,
    managed: bool,
    launch_mode: String,
    start_target: Option<String>,
    playit_enabled: bool,
    heavy_storage_directory: Option<String>,
}

impl Default for ServerProfile {
    fn default() -> Self {
        Self {
            id: String::from("managed-default"),
            name: String::from("Meu servidor"),
            version: String::from("26.1.2"),
            software: String::from("vanilla"),
            memory_gb: 4,
            port: 25565,
            max_players: 12,
            motd: String::from("Servidor criado pelo VEX Launcher"),
            online_mode: true,
            gamemode: String::from("survival"),
            difficulty: String::from("normal"),
            directory: storage_root()
                .join("servers")
                .join("Meu servidor")
                .to_string_lossy()
                .into_owned(),
            managed: true,
            launch_mode: String::from("managed"),
            start_target: None,
            playit_enabled: false,
            heavy_storage_directory: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
struct ServerRegistry {
    active_id: String,
    servers: Vec<ServerProfile>,
}

#[derive(Debug, Clone, Serialize, Default)]
struct ServerMetrics {
    cpu_percent: f32,
    memory_mb: f64,
    system_memory_used_mb: f64,
    system_memory_total_mb: f64,
    process_count: usize,
}

#[derive(Debug, Serialize)]
struct ServerStatus {
    running: bool,
    ready: bool,
    startup_percent: u8,
    startup_label: String,
    pid: Option<u32>,
    control_mode: String,
    recovered: bool,
    detail: String,
    profile: ServerProfile,
    log_path: String,
    metrics: ServerMetrics,
}

#[derive(Debug, Clone, Serialize)]
struct ServerProfileRuntimeSummary {
    profile_id: String,
    running: bool,
    pid: Option<u32>,
    control_mode: String,
}

#[derive(Debug, Clone, Serialize)]
struct PlayitStatus {
    installed: bool,
    running: bool,
    service_running: bool,
    authenticated: bool,
    needs_setup: bool,
    needs_group_access: bool,
    detail: String,
}

#[derive(Debug, Clone, Serialize)]
struct ServerContentItem {
    name: String,
    path: String,
    kind: String,
    side: String,
    enabled: bool,
    size_mb: f64,
}

#[derive(Debug, Serialize)]
struct ServerPackExportResult {
    path: String,
    included_files: usize,
    skipped_files: usize,
    warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
struct ServerPlayableInstanceResult {
    instance: InstalledInstance,
    included_files: usize,
    skipped_files: usize,
    warnings: Vec<String>,
    server_address: String,
}

#[derive(Debug, Clone, Serialize)]
struct ServerMapStatus {
    provider: Option<String>,
    providers: Vec<String>,
    web_url: Option<String>,
    bluemap_web_url: Option<String>,
    bluemap_online: bool,
    port: Option<u16>,
    online: bool,
    chunky_installed: bool,
    bluemap_installed: bool,
    bluemap_enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    bluemap_pending_enabled: Option<bool>,
    bluemap_render_threads: u32,
    bluemap_max_threads: u32,
    bluemap_configured: bool,
    bluemap_storage_safe: bool,
    bluemap_storage_message: String,
    world_name: String,
    world_directory: String,
    heavy_storage_directory: Option<String>,
    storage_safe: bool,
    storage_message: String,
    world_generation_safe: bool,
    world_generation_message: String,
    world_available_disk_gb: f64,
    available_disk_gb: f64,
}

#[derive(Debug, Clone, Serialize)]
struct ServerMapPlayer {
    name: String,
    uuid: String,
    world: String,
    x: Option<f64>,
    y: Option<f64>,
    z: Option<f64>,
    yaw: Option<f64>,
    health: Option<f64>,
    armor: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
struct BlueMapAreaConfig {
    map: String,
    x: i64,
    z: i64,
    radius_blocks: u32,
    radius_chunks: u32,
    config_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ServerMapTaskRecord {
    operation: String,
    provider: String,
    label: String,
    started_unix: u64,
    active: bool,
    native_log_offset: u64,
    vex_log_offset: u64,
    #[serde(default)]
    last_probe_unix: u64,
    #[serde(default)]
    auto_stop: bool,
}

#[derive(Debug, Clone, Serialize)]
struct ServerMapTaskProgress {
    operation: String,
    provider: String,
    active: bool,
    determinate: bool,
    percent: Option<f32>,
    label: String,
    detail: String,
    eta_seconds: Option<u64>,
    started_unix: Option<u64>,
}

#[derive(Debug, Serialize)]
struct ChunkGenerationPreview {
    command: String,
    chunks: u64,
    estimated_disk_gb: f64,
    available_disk_gb: f64,
    safe: bool,
    message: String,
}

#[derive(Debug, Clone)]
struct ServerToolArtifact {
    project: String,
    version: String,
    filename: String,
    download_url: String,
    sha512: String,
    size: u64,
    license: String,
}

#[derive(Debug, Clone, Serialize)]
struct ServerToolInstallItem {
    project: String,
    version: String,
    filename: String,
    destination: String,
    license: String,
}

#[derive(Debug, Clone, Serialize)]
struct ServerToolInstallResult {
    installed: Vec<ServerToolInstallItem>,
    already_present: Vec<String>,
    source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ServerSupervisorSpec {
    profile_id: String,
    program: String,
    arguments: Vec<String>,
    working_directory: String,
    log_path: String,
    record_path: String,
    commands_directory: String,
    heartbeat_path: String,
    stop_when_vex_exits: bool,
    #[serde(default)]
    startup_commands: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ServerRuntimeRecord {
    profile_id: String,
    supervisor_pid: u32,
    server_pid: Option<u32>,
    status: String,
    working_directory: String,
    log_path: String,
    commands_directory: String,
    heartbeat_path: String,
    started_unix: u64,
    exit_code: Option<i32>,
    error: Option<String>,
}

static SERVER_OPERATION_ACTIVE: AtomicBool = AtomicBool::new(false);
static SERVER_REGISTRY_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
static SERVER_LOG_WRITE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
static SERVER_COMMAND_SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static SERVER_HEARTBEAT_STARTED: AtomicBool = AtomicBool::new(false);
static TRAY_AVAILABLE: AtomicBool = AtomicBool::new(false);
static APP_EXPLICIT_EXIT: AtomicBool = AtomicBool::new(false);

const SERVER_LOG_MAX_BYTES: u64 = 32 * 1024 * 1024;
const SERVER_LOG_RETAIN_BYTES: u64 = 8 * 1024 * 1024;
const SERVER_LOG_READ_BYTES: u64 = 1024 * 1024;

struct ServerOperationGuard;

impl Drop for ServerOperationGuard {
    fn drop(&mut self) {
        SERVER_OPERATION_ACTIVE.store(false, Ordering::Release);
    }
}

fn begin_server_operation() -> Result<ServerOperationGuard, String> {
    SERVER_OPERATION_ACTIVE
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .map(|_| ServerOperationGuard)
        .map_err(|_| {
            String::from(
                "Outra operação do servidor ainda está em andamento. Aguarde ela terminar.",
            )
        })
}

fn server_registry_lock() -> &'static Mutex<()> {
    SERVER_REGISTRY_LOCK.get_or_init(|| Mutex::new(()))
}

fn server_log_write_lock() -> &'static Mutex<()> {
    SERVER_LOG_WRITE_LOCK.get_or_init(|| Mutex::new(()))
}

#[cfg(target_os = "linux")]
#[derive(Clone, Copy)]
struct ServerCpuSample {
    pid: u32,
    total_ticks: u64,
    process_ticks: u64,
}

#[cfg(target_os = "linux")]
static SERVER_CPU_SAMPLE: OnceLock<Mutex<Option<ServerCpuSample>>> = OnceLock::new();

fn server_profile_path() -> PathBuf {
    storage_root().join("servers").join("server-profile.json")
}

fn server_registry_path() -> PathBuf {
    storage_root().join("servers").join("servers.json")
}

fn server_log_path_for(profile: &ServerProfile) -> PathBuf {
    let safe_id: String = profile
        .id
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
        .take(96)
        .collect();
    storage_root().join("logs").join("servers").join(format!(
        "{}.log",
        if safe_id.is_empty() {
            "server"
        } else {
            &safe_id
        }
    ))
}

fn server_log_path() -> PathBuf {
    server_log_path_for(&read_server_profile())
}

fn safe_server_profile_id(profile: &ServerProfile) -> String {
    let safe: String = profile
        .id
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
        .take(96)
        .collect();
    if safe.is_empty() {
        String::from("server")
    } else {
        safe
    }
}

fn server_runtime_directory(profile: &ServerProfile) -> PathBuf {
    storage_root()
        .join("servers")
        .join("runtime")
        .join(safe_server_profile_id(profile))
}

fn server_runtime_record_path(profile: &ServerProfile) -> PathBuf {
    server_runtime_directory(profile).join("state.json")
}

fn server_runtime_spec_path(profile: &ServerProfile) -> PathBuf {
    server_runtime_directory(profile).join("supervisor.json")
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

#[cfg(target_os = "linux")]
fn process_is_alive(pid: u32) -> bool {
    pid > 0 && Path::new("/proc").join(pid.to_string()).is_dir()
}

#[cfg(windows)]
fn process_is_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    hidden_command("tasklist.exe")
        .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).contains(&pid.to_string()))
        .unwrap_or(false)
}

#[cfg(all(not(target_os = "linux"), not(windows)))]
fn process_is_alive(_pid: u32) -> bool {
    false
}

fn read_server_runtime_record(profile: &ServerProfile) -> Option<ServerRuntimeRecord> {
    read_json_with_recovery(&server_runtime_record_path(profile))
}

fn active_server_runtime_record(profile: &ServerProfile) -> Option<ServerRuntimeRecord> {
    let record = read_server_runtime_record(profile)?;
    if !matches!(record.status.as_str(), "starting" | "running" | "stopping") {
        return None;
    }
    process_is_alive(record.supervisor_pid).then_some(record)
}

fn write_server_runtime_record(path: &Path, record: &ServerRuntimeRecord) -> Result<(), String> {
    replace_text_file(
        path,
        &serde_json::to_string_pretty(record).map_err(|error| error.to_string())?,
    )
}

fn write_runtime_heartbeat(path: &Path) -> Result<(), String> {
    replace_text_file(path, &format!("{}\n", unix_seconds()))
}

fn start_server_heartbeat_thread() {
    if SERVER_HEARTBEAT_STARTED.swap(true, Ordering::AcqRel) {
        return;
    }
    let runtime_root = storage_root().join("servers").join("runtime");
    std::thread::spawn(move || loop {
        if let Ok(entries) = fs::read_dir(&runtime_root) {
            for entry in entries.flatten() {
                let state_path = entry.path().join("state.json");
                let Some(record) = read_json_with_recovery::<ServerRuntimeRecord>(&state_path)
                else {
                    continue;
                };
                if matches!(record.status.as_str(), "starting" | "running" | "stopping")
                    && process_is_alive(record.supervisor_pid)
                {
                    let _ = write_runtime_heartbeat(Path::new(&record.heartbeat_path));
                }
            }
        }
        std::thread::sleep(Duration::from_secs(2));
    });
}

fn queue_server_command(record: &ServerRuntimeRecord, command: &str) -> Result<(), String> {
    let clean = command.trim();
    if clean.is_empty() {
        return Ok(());
    }
    if clean.len() > 4096 || clean.contains(['\n', '\r']) {
        return Err(String::from(
            "O comando do servidor é inválido ou grande demais.",
        ));
    }
    let directory = PathBuf::from(&record.commands_directory);
    fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    let sequence = SERVER_COMMAND_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let path = directory.join(format!("{nanos:032}-{sequence:016}.cmd"));
    replace_text_file(&path, &format!("{clean}\n"))
}

fn pending_supervisor_commands(directory: &Path) -> Vec<PathBuf> {
    let mut commands: Vec<PathBuf> = fs::read_dir(directory)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("cmd"))
        .collect();
    commands.sort();
    commands
}

fn run_server_supervisor(spec_path: &Path) -> Result<(), String> {
    let spec: ServerSupervisorSpec = read_json_with_recovery(spec_path)
        .ok_or_else(|| String::from("Especificação do supervisor ausente ou inválida."))?;
    let record_path = PathBuf::from(&spec.record_path);
    let commands_directory = PathBuf::from(&spec.commands_directory);
    let heartbeat_path = PathBuf::from(&spec.heartbeat_path);
    fs::create_dir_all(&commands_directory).map_err(|error| error.to_string())?;
    let log_path = PathBuf::from(&spec.log_path);
    fs::create_dir_all(
        log_path
            .parent()
            .ok_or_else(|| String::from("Caminho de log do servidor inválido."))?,
    )
    .map_err(|error| error.to_string())?;
    let stdout = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .map_err(|error| error.to_string())?;
    let stderr = stdout.try_clone().map_err(|error| error.to_string())?;
    let mut command = hidden_command(&spec.program);
    command
        .args(&spec.arguments)
        .current_dir(&spec.working_directory)
        .stdin(Stdio::piped())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    let started_unix = unix_seconds();
    let mut record = ServerRuntimeRecord {
        profile_id: spec.profile_id.clone(),
        supervisor_pid: std::process::id(),
        server_pid: None,
        status: String::from("starting"),
        working_directory: spec.working_directory.clone(),
        log_path: spec.log_path.clone(),
        commands_directory: spec.commands_directory.clone(),
        heartbeat_path: spec.heartbeat_path.clone(),
        started_unix,
        exit_code: None,
        error: None,
    };
    write_server_runtime_record(&record_path, &record)?;
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            record.status = String::from("failed");
            record.error = Some(error.to_string());
            let _ = write_server_runtime_record(&record_path, &record);
            return Err(error.to_string());
        }
    };
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| String::from("O supervisor não conseguiu abrir o console."))?;
    record.server_pid = Some(child.id());
    record.status = String::from("running");
    write_server_runtime_record(&record_path, &record)?;
    let mut stop_sent = false;
    let mut startup_commands_sent = spec.startup_commands.is_empty();
    loop {
        if !startup_commands_sent {
            let log_tail = read_text_tail(&log_path, 96 * 1024).to_ascii_lowercase();
            if log_tail.contains("bluemap loaded!") || log_tail.contains("bluemap carregado") {
                for command in &spec.startup_commands {
                    if command.len() <= 4096 && !command.contains(['\n', '\r']) {
                        let _ = writeln!(stdin, "{command}");
                    }
                }
                let _ = stdin.flush();
                startup_commands_sent = true;
            }
        }
        for path in pending_supervisor_commands(&commands_directory) {
            let command = fs::read_to_string(&path).unwrap_or_default();
            let clean = command.trim();
            if !clean.is_empty() && clean.len() <= 4096 && !clean.contains(['\n', '\r']) {
                if writeln!(stdin, "{clean}")
                    .and_then(|_| stdin.flush())
                    .is_err()
                {
                    record.error = Some(String::from("O console do servidor deixou de responder."));
                }
                if clean.eq_ignore_ascii_case("stop") {
                    stop_sent = true;
                    record.status = String::from("stopping");
                    let _ = write_server_runtime_record(&record_path, &record);
                }
            }
            let _ = fs::remove_file(path);
        }
        if spec.stop_when_vex_exits && !stop_sent && unix_seconds() > started_unix + 20 {
            let heartbeat = fs::read_to_string(&heartbeat_path)
                .ok()
                .and_then(|value| value.trim().parse::<u64>().ok())
                .unwrap_or_default();
            if unix_seconds().saturating_sub(heartbeat) > 12 {
                let _ = writeln!(stdin, "stop");
                let _ = stdin.flush();
                stop_sent = true;
                record.status = String::from("stopping");
                record.error = Some(String::from(
                    "O VEX deixou de responder; encerramento seguro solicitado automaticamente.",
                ));
                let _ = write_server_runtime_record(&record_path, &record);
            }
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                record.status = String::from("stopped");
                record.exit_code = status.code();
                write_server_runtime_record(&record_path, &record)?;
                write_server_log_line(&log_path, "Servidor finalizado.");
                return Ok(());
            }
            Ok(None) => {}
            Err(error) => {
                record.status = String::from("failed");
                record.error = Some(error.to_string());
                let _ = write_server_runtime_record(&record_path, &record);
                return Err(error.to_string());
            }
        }
        std::thread::sleep(Duration::from_millis(120));
    }
}

pub fn run_server_supervisor_from_args() -> bool {
    let mut arguments = env::args_os();
    let _program = arguments.next();
    if arguments.next().as_deref() != Some(std::ffi::OsStr::new("--vex-server-supervisor")) {
        return false;
    }
    let result = arguments
        .next()
        .map(PathBuf::from)
        .ok_or_else(|| String::from("Caminho do supervisor não informado."))
        .and_then(|path| run_server_supervisor(&path));
    if let Err(error) = result {
        eprintln!("Falha no supervisor do servidor VEX: {error}");
    }
    true
}

fn read_small_text(path: &Path, limit: u64) -> Option<String> {
    let file = fs::File::open(path).ok()?;
    let mut content = String::new();
    file.take(limit).read_to_string(&mut content).ok()?;
    Some(content)
}

fn read_server_properties(directory: &Path) -> HashMap<String, String> {
    read_small_text(&directory.join("server.properties"), 1024 * 1024)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| {
            let clean = line.trim();
            if clean.is_empty() || clean.starts_with('#') {
                return None;
            }
            let (key, value) = clean.split_once('=')?;
            Some((key.trim().to_owned(), value.trim().to_owned()))
        })
        .collect()
}

fn first_existing_server_file(
    directory: &Path,
    preferred: &[&str],
    extension: &str,
) -> Option<PathBuf> {
    for name in preferred {
        let candidate = directory.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    let mut candidates: Vec<PathBuf> = fs::read_dir(directory)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .extension()
                    .and_then(|value| value.to_str())
                    .is_some_and(|value| value.eq_ignore_ascii_case(extension))
                && !path
                    .file_name()
                    .and_then(|value| value.to_str())
                    .unwrap_or_default()
                    .to_ascii_lowercase()
                    .contains("installer")
        })
        .collect();
    candidates.sort_by_key(|path| {
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let priority = if name.starts_with("server") {
            0
        } else if name.contains("paper") || name.contains("purpur") || name.contains("fabric") {
            1
        } else if name.contains("forge") || name.contains("neoforge") {
            2
        } else {
            3
        };
        (priority, name)
    });
    candidates.into_iter().next()
}

fn detected_server_software(directory: &Path, target: Option<&Path>) -> String {
    let target_name = target
        .and_then(Path::file_name)
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let script_content = target
        .filter(|path| {
            path.extension()
                .and_then(|value| value.to_str())
                .is_some_and(|value| {
                    matches!(value.to_ascii_lowercase().as_str(), "sh" | "bat" | "cmd")
                })
        })
        .and_then(|path| read_small_text(path, 1024 * 1024))
        .unwrap_or_default()
        .to_ascii_lowercase();
    let root_jars = fs::read_dir(directory)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|entry| {
            entry
                .path()
                .extension()
                .and_then(|value| value.to_str())
                .is_some_and(|value| value.eq_ignore_ascii_case("jar"))
                .then(|| entry.file_name().to_string_lossy().to_ascii_lowercase())
        })
        .collect::<Vec<_>>()
        .join(" ");
    let loader_evidence = format!("{target_name} {script_content} {root_jars}");
    if directory.join("plugins").is_dir()
        || ["paper", "purpur", "spigot", "bukkit"]
            .iter()
            .any(|name| loader_evidence.contains(name))
    {
        String::from("paper")
    } else if loader_evidence.contains("neoforge") {
        String::from("neoforge")
    } else if loader_evidence.contains("forge") {
        String::from("forge")
    } else if loader_evidence.contains("fabric")
        || directory.join("fabric-server-launch.jar").is_file()
    {
        String::from("fabric")
    } else if loader_evidence.contains("quilt") {
        String::from("quilt")
    } else if directory.join("mods").is_dir() {
        String::from("modded")
    } else {
        String::from("vanilla")
    }
}

fn detected_server_version(directory: &Path, target: Option<&Path>) -> String {
    if let Some(log) = read_small_text(&directory.join("logs").join("latest.log"), 4 * 1024 * 1024)
    {
        for line in log.lines().rev().take(2000) {
            if let Some(value) = line.split("Starting minecraft server version ").nth(1) {
                let version = value.split_whitespace().next().unwrap_or_default();
                if !version.is_empty() {
                    return version
                        .trim_matches(|character: char| {
                            !character.is_ascii_alphanumeric()
                                && character != '.'
                                && character != '-'
                                && character != '_'
                        })
                        .to_owned();
                }
            }
            if let Some(value) = line.split("(MC: ").nth(1) {
                let version = value.split(')').next().unwrap_or_default().trim();
                if !version.is_empty() {
                    return version.to_owned();
                }
            }
            if let Some(value) = line.split("Loading Minecraft ").nth(1) {
                let version = value.split_whitespace().next().unwrap_or_default();
                if !version.is_empty() {
                    return version.to_owned();
                }
            }
        }
    }
    if let Some(name) = target
        .and_then(Path::file_name)
        .and_then(|value| value.to_str())
    {
        for part in name.trim_end_matches(".jar").split(['-', '_']) {
            if part.starts_with(|character: char| character.is_ascii_digit()) && part.contains('.')
            {
                return part.to_owned();
            }
        }
    }
    String::from("desconhecida")
}

fn detected_memory_gb(script: Option<&Path>) -> u32 {
    let Some(content) = script.and_then(|path| read_small_text(path, 1024 * 1024)) else {
        return 4;
    };
    let Some((_, after)) = content.split_once("-Xmx") else {
        return 4;
    };
    let value: String = after
        .chars()
        .take_while(|character| {
            character.is_ascii_digit() || matches!(character, 'g' | 'G' | 'm' | 'M')
        })
        .collect();
    let (number, unit) = value.split_at(
        value
            .find(|character: char| !character.is_ascii_digit())
            .unwrap_or(value.len()),
    );
    let amount = number.parse::<u32>().unwrap_or(4);
    match unit.to_ascii_lowercase().as_str() {
        "m" => amount.div_ceil(1024).max(1),
        "g" => amount.max(1),
        _ => 4,
    }
}

fn resolved_server_target(profile: &ServerProfile) -> Result<PathBuf, String> {
    let directory = fs::canonicalize(&profile.directory)
        .map_err(|_| String::from("A pasta importada do servidor não existe mais."))?;
    let target = profile
        .start_target
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            String::from("O VEX não encontrou o arquivo usado para iniciar este servidor.")
        })?;
    let target_path = PathBuf::from(target);
    let joined = if target_path.is_absolute() {
        target_path
    } else {
        directory.join(target_path)
    };
    let canonical = fs::canonicalize(joined).map_err(|_| {
        String::from("O script ou JAR configurado para o servidor não existe mais.")
    })?;
    if !canonical.starts_with(&directory) || !canonical.is_file() {
        return Err(String::from(
            "O arquivo de inicialização precisa ficar dentro da pasta do servidor.",
        ));
    }
    Ok(canonical)
}

#[cfg(target_os = "linux")]
fn linux_process_stat(pid: u32) -> Option<(u32, u64)> {
    let raw = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let close = raw.rfind(") ")?;
    let fields: Vec<&str> = raw[(close + 2)..].split_whitespace().collect();
    if fields.len() <= 12 {
        return None;
    }
    let parent = fields[1].parse().ok()?;
    let user_ticks: u64 = fields[11].parse().ok()?;
    let system_ticks: u64 = fields[12].parse().ok()?;
    Some((parent, user_ticks.saturating_add(system_ticks)))
}

#[cfg(target_os = "linux")]
fn linux_process_memory_kb(pid: u32) -> u64 {
    fs::read_to_string(format!("/proc/{pid}/status"))
        .ok()
        .and_then(|content| {
            content.lines().find_map(|line| {
                let value = line.strip_prefix("VmRSS:")?.trim();
                value.split_whitespace().next()?.parse().ok()
            })
        })
        .unwrap_or_default()
}

#[cfg(target_os = "linux")]
fn linux_system_cpu_ticks() -> u64 {
    fs::read_to_string("/proc/stat")
        .ok()
        .and_then(|content| content.lines().next().map(str::to_owned))
        .map(|line| {
            line.split_whitespace()
                .skip(1)
                .filter_map(|value| value.parse::<u64>().ok())
                .fold(0_u64, u64::saturating_add)
        })
        .unwrap_or_default()
}

#[cfg(target_os = "linux")]
fn linux_system_memory_mb() -> (f64, f64) {
    let content = fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let mut total_kb = 0_u64;
    let mut available_kb = 0_u64;
    for line in content.lines() {
        if let Some(value) = line.strip_prefix("MemTotal:") {
            total_kb = value
                .split_whitespace()
                .next()
                .and_then(|value| value.parse().ok())
                .unwrap_or_default();
        } else if let Some(value) = line.strip_prefix("MemAvailable:") {
            available_kb = value
                .split_whitespace()
                .next()
                .and_then(|value| value.parse().ok())
                .unwrap_or_default();
        }
    }
    (
        total_kb.saturating_sub(available_kb) as f64 / 1024.0,
        total_kb as f64 / 1024.0,
    )
}

#[cfg(target_os = "linux")]
fn server_metrics(pid: Option<u32>) -> ServerMetrics {
    let (system_memory_used_mb, system_memory_total_mb) = linux_system_memory_mb();
    let Some(root_pid) = pid else {
        if let Ok(mut sample) = SERVER_CPU_SAMPLE.get_or_init(|| Mutex::new(None)).lock() {
            *sample = None;
        }
        return ServerMetrics {
            system_memory_used_mb,
            system_memory_total_mb,
            ..ServerMetrics::default()
        };
    };

    let mut stats = HashMap::<u32, (u32, u64)>::new();
    if let Ok(entries) = fs::read_dir("/proc") {
        for entry in entries.flatten() {
            let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|value| value.parse::<u32>().ok())
            else {
                continue;
            };
            if let Some(stat) = linux_process_stat(pid) {
                stats.insert(pid, stat);
            }
        }
    }
    let mut queue = VecDeque::from([root_pid]);
    let mut process_ids = Vec::new();
    while let Some(current) = queue.pop_front() {
        if process_ids.contains(&current) {
            continue;
        }
        process_ids.push(current);
        for (&candidate, &(parent, _)) in &stats {
            if parent == current {
                queue.push_back(candidate);
            }
        }
    }
    let process_ticks = process_ids
        .iter()
        .filter_map(|pid| stats.get(pid).map(|(_, ticks)| *ticks))
        .fold(0_u64, u64::saturating_add);
    let memory_kb = process_ids
        .iter()
        .map(|pid| linux_process_memory_kb(*pid))
        .fold(0_u64, u64::saturating_add);
    let total_ticks = linux_system_cpu_ticks();
    let current = ServerCpuSample {
        pid: root_pid,
        total_ticks,
        process_ticks,
    };
    let cpu_percent = SERVER_CPU_SAMPLE
        .get_or_init(|| Mutex::new(None))
        .lock()
        .map(|mut previous| {
            let percent = previous
                .filter(|sample| sample.pid == root_pid)
                .and_then(|sample| {
                    let total_delta = total_ticks.saturating_sub(sample.total_ticks);
                    let process_delta = process_ticks.saturating_sub(sample.process_ticks);
                    (total_delta > 0).then(|| {
                        let cores = std::thread::available_parallelism()
                            .map(|value| value.get())
                            .unwrap_or(1) as f32;
                        process_delta as f32 / total_delta as f32 * 100.0 * cores
                    })
                })
                .unwrap_or_default();
            *previous = Some(current);
            percent
        })
        .unwrap_or_default();
    ServerMetrics {
        cpu_percent,
        memory_mb: memory_kb as f64 / 1024.0,
        system_memory_used_mb,
        system_memory_total_mb,
        process_count: process_ids.len(),
    }
}

#[cfg(not(target_os = "linux"))]
fn server_metrics(_pid: Option<u32>) -> ServerMetrics {
    ServerMetrics::default()
}

#[cfg(target_os = "linux")]
fn playit_group_membership_configured() -> bool {
    let user = env::var("USER").unwrap_or_default();
    if user.is_empty() {
        return false;
    }
    hidden_command("getent")
        .args(["group", "playit"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
        .and_then(|line| line.rsplit(':').next().map(str::to_owned))
        .is_some_and(|members| members.split(',').any(|member| member.trim() == user))
}

#[cfg(target_os = "linux")]
fn playit_linux_cli_output(action: &str) -> std::io::Result<std::process::Output> {
    if playit_group_membership_configured() {
        hidden_command("sg")
            .args(["playit", "-c", &format!("playit {action}")])
            .output()
    } else {
        hidden_command("playit").arg(action).output()
    }
}

fn playit_status_internal() -> PlayitStatus {
    #[cfg(target_os = "linux")]
    {
        let unit_installed = Path::new("/usr/lib/systemd/system/playit.service").is_file()
            || Path::new("/etc/systemd/system/playit.service").is_file();
        let binary_installed = hidden_command("playit").arg("version").output().is_ok();
        if !unit_installed && !binary_installed {
            return PlayitStatus {
                installed: false,
                running: false,
                service_running: false,
                authenticated: false,
                needs_setup: false,
                needs_group_access: false,
                detail: String::from("Playit não está instalado neste computador."),
            };
        }
        let service_running = hidden_command("systemctl")
            .args(["is-active", "--quiet", "playit.service"])
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
        let needs_group_access = unit_installed && !playit_group_membership_configured();
        if needs_group_access {
            return PlayitStatus {
                installed: true,
                running: false,
                service_running,
                authenticated: false,
                needs_setup: false,
                needs_group_access: true,
                detail: String::from(
                    "O serviço está instalado, mas o usuário ainda precisa receber acesso ao grupo playit.",
                ),
            };
        }
        let output = match playit_linux_cli_output("status") {
            Ok(output) => output,
            Err(error) => {
                return PlayitStatus {
                    installed: true,
                    running: false,
                    service_running,
                    authenticated: false,
                    needs_setup: false,
                    needs_group_access: false,
                    detail: format!("Não foi possível consultar o Playit: {error}"),
                }
            }
        };
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let normalized = text.to_ascii_lowercase();
        let needs_setup = normalized.contains("waiting for secret")
            || normalized.contains("secret configured: false");
        let authenticated = normalized.contains("secret configured: true") && !needs_setup;
        let phase_connected = normalized.lines().any(|line| {
            line.trim_start().starts_with("phase:")
                && !line.contains("waiting for secret")
                && !line.contains("stopped")
        });
        let running = service_running && authenticated && phase_connected;
        let detail = if needs_setup {
            String::from("Agente iniciado, aguardando vincular sua conta do playit.gg.")
        } else if running {
            String::from("Agente autenticado e túnel do Playit em execução.")
        } else if service_running {
            String::from("Serviço do Playit ativo; aguardando conexão do agente.")
        } else {
            String::from("Playit instalado, mas o serviço está parado.")
        };
        PlayitStatus {
            installed: true,
            running,
            service_running,
            authenticated,
            needs_setup,
            needs_group_access: false,
            detail,
        }
    }
    #[cfg(not(target_os = "linux"))]
    match hidden_command("playit").arg("status").output() {
        Ok(output) => {
            let text = format!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            let normalized = text.to_ascii_lowercase();
            let running = output.status.success()
                && !normalized.contains("not running")
                && normalized.contains("running");
            PlayitStatus {
                installed: true,
                running,
                service_running: running,
                authenticated: running,
                needs_setup: false,
                needs_group_access: false,
                detail: text
                    .lines()
                    .find(|line| !line.trim().is_empty())
                    .unwrap_or("Playit respondeu sem detalhes.")
                    .trim()
                    .to_owned(),
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => PlayitStatus {
            installed: false,
            running: false,
            service_running: false,
            authenticated: false,
            needs_setup: false,
            needs_group_access: false,
            detail: String::from("Playit não está instalado neste computador."),
        },
        Err(error) => PlayitStatus {
            installed: true,
            running: false,
            service_running: false,
            authenticated: false,
            needs_setup: false,
            needs_group_access: false,
            detail: format!("Não foi possível consultar o Playit: {error}"),
        },
    }
}

#[tauri::command]
fn playit_status() -> PlayitStatus {
    playit_status_internal()
}

fn run_playit_action(action: &str) -> Result<PlayitStatus, String> {
    #[cfg(target_os = "linux")]
    {
        let unit_installed = Path::new("/usr/lib/systemd/system/playit.service").is_file()
            || Path::new("/etc/systemd/system/playit.service").is_file();
        if unit_installed {
            let status = if action == "start" {
                playit_linux_cli_output("start")
                    .map(|output| output.status)
                    .map_err(|error| error.to_string())?
            } else {
                hidden_command("pkexec")
                    .args(["systemctl", action, "playit.service"])
                    .status()
                    .map_err(|error| error.to_string())?
            };
            if status.success() {
                return Ok(playit_status_internal());
            }
            return Err(format!(
                "O Linux não autorizou '{} playit.service'. Execute a ação novamente após liberar o serviço no sistema.",
                action
            ));
        }
    }
    let output = hidden_command("playit")
        .arg(action)
        .output()
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                String::from("Playit não está instalado. Abra o tutorial de rede para instalar.")
            } else {
                error.to_string()
            }
        })?;
    if !output.status.success() {
        let detail = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return Err(detail.trim().to_owned());
    }
    Ok(playit_status_internal())
}

#[tauri::command]
fn repair_playit_access() -> Result<PlayitStatus, String> {
    #[cfg(target_os = "linux")]
    {
        let user = env::var("USER").map_err(|_| String::from("Usuário Linux não identificado."))?;
        if user.is_empty()
            || !user.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '_' | '-')
            })
        {
            return Err(String::from("Nome de usuário Linux inválido."));
        }
        let status = hidden_command("pkexec")
            .args(["/usr/sbin/usermod", "-aG", "playit", &user])
            .status()
            .map_err(|error| format!("Não foi possível abrir a autorização do Linux: {error}"))?;
        if !status.success() {
            return Err(String::from(
                "A autorização do Linux foi cancelada ou recusada.",
            ));
        }
        Ok(playit_status_internal())
    }
    #[cfg(not(target_os = "linux"))]
    Err(String::from(
        "Este reparo automático é necessário apenas no Linux.",
    ))
}

#[tauri::command]
fn setup_playit() -> Result<String, String> {
    #[cfg(target_os = "linux")]
    {
        if !playit_group_membership_configured() {
            return Err(String::from(
                "Libere primeiro o acesso do VEX ao grupo playit.",
            ));
        }
        if !playit_status_internal().service_running {
            run_playit_action("start")?;
        }
        let mut child = hidden_command("sg")
            .args(["playit", "-c", "playit setup"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| format!("Não foi possível iniciar a vinculação: {error}"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| String::from("O Playit não forneceu o link de vinculação."))?;
        let mut reader = BufReader::new(stdout);
        let mut url = String::new();
        for _ in 0..8 {
            let mut line = String::new();
            if reader
                .read_line(&mut line)
                .map_err(|error| error.to_string())?
                == 0
            {
                break;
            }
            if let Some(found) = line
                .split_whitespace()
                .find(|value| value.starts_with("https://playit.gg/claim/"))
            {
                url = found.trim().to_owned();
                break;
            }
        }
        if url.is_empty() {
            let _ = child.kill();
            return Err(String::from("O Playit não gerou uma página de vinculação."));
        }
        std::thread::spawn(move || {
            let mut ignored = String::new();
            while reader.read_line(&mut ignored).unwrap_or_default() > 0 {
                ignored.clear();
            }
            let _ = child.wait();
        });
        Ok(url)
    }
    #[cfg(not(target_os = "linux"))]
    Err(String::from(
        "Abra o Playit instalado para vincular sua conta neste sistema.",
    ))
}

#[tauri::command]
fn start_playit() -> Result<PlayitStatus, String> {
    if playit_status_internal().running {
        return Ok(playit_status_internal());
    }
    run_playit_action("start")
}

#[tauri::command]
fn stop_playit() -> Result<PlayitStatus, String> {
    let status = playit_status_internal();
    if !status.service_running {
        return Ok(status);
    }
    run_playit_action("stop")
}

fn current_server_directory(profile: &ServerProfile) -> Result<PathBuf, String> {
    let directory = fs::canonicalize(&profile.directory)
        .map_err(|_| String::from("A pasta do servidor não existe ou não está montada."))?;
    if !directory.is_dir() {
        return Err(String::from("O caminho do servidor não é uma pasta."));
    }
    if profile.managed {
        let root = fs::canonicalize(storage_root().join("servers"))
            .unwrap_or_else(|_| storage_root().join("servers"));
        if !directory.starts_with(root) {
            return Err(String::from(
                "O servidor gerenciado está fora da pasta segura do VEX.",
            ));
        }
    }
    Ok(directory)
}

fn jar_mod_side(path: &Path) -> String {
    let Ok(file) = fs::File::open(path) else {
        return String::from("unknown");
    };
    let Ok(mut archive) = zip::ZipArchive::new(file) else {
        return String::from("unknown");
    };
    let fabric_environment = {
        let Ok(entry) = archive.by_name("fabric.mod.json") else {
            return String::from("unknown");
        };
        let mut content = String::new();
        if entry
            .take(1024 * 1024)
            .read_to_string(&mut content)
            .is_err()
        {
            return String::from("unknown");
        }
        serde_json::from_str::<Value>(&content)
            .ok()
            .and_then(|value| {
                value
                    .get("environment")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
    };
    match fabric_environment.as_deref() {
        Some("server") => String::from("server-only"),
        Some("client") => String::from("client-only"),
        Some("*") => String::from("client-required"),
        _ => String::from("unknown"),
    }
}

fn server_content_items(profile: &ServerProfile) -> Result<Vec<ServerContentItem>, String> {
    let directory = current_server_directory(profile)?;
    let mut items = Vec::new();
    for (folder, kind) in [("mods", "mod"), ("plugins", "plugin")] {
        let content_directory = directory.join(folder);
        let Ok(entries) = fs::read_dir(content_directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let name = path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or_default()
                .to_owned();
            let normalized = name.to_ascii_lowercase();
            if !normalized.ends_with(".jar") && !normalized.ends_with(".jar.disabled") {
                continue;
            }
            let enabled = normalized.ends_with(".jar");
            let side = if kind == "plugin" {
                String::from("server-only")
            } else {
                jar_mod_side(&path)
            };
            let size_mb = fs::metadata(&path)
                .map(|metadata| metadata.len() as f64 / 1024.0 / 1024.0)
                .unwrap_or_default();
            items.push(ServerContentItem {
                name,
                path: path.to_string_lossy().into_owned(),
                kind: kind.to_owned(),
                side,
                enabled,
                size_mb,
            });
        }
    }
    items.sort_by(|left, right| {
        left.kind.cmp(&right.kind).then_with(|| {
            left.name
                .to_ascii_lowercase()
                .cmp(&right.name.to_ascii_lowercase())
        })
    });
    Ok(items)
}

#[tauri::command]
fn list_server_content() -> Result<Vec<ServerContentItem>, String> {
    server_content_items(&read_server_profile())
}

#[tauri::command]
fn set_server_content_enabled(
    path: String,
    enabled: bool,
) -> Result<Vec<ServerContentItem>, String> {
    let _operation = begin_server_operation()?;
    ensure_server_stopped_for_profile_change()?;
    let profile = read_server_profile();
    let server_directory = current_server_directory(&profile)?;
    let requested = PathBuf::from(path.trim());
    let metadata = fs::symlink_metadata(&requested)
        .map_err(|_| String::from("O arquivo de conteúdo não existe mais."))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(String::from(
            "Somente arquivos JAR reais podem ser ativados ou desativados.",
        ));
    }
    let parent = requested
        .parent()
        .and_then(|path| fs::canonicalize(path).ok())
        .ok_or_else(|| String::from("Pasta de conteúdo inválida."))?;
    let allowed_parent = ["mods", "plugins"].iter().any(|folder| {
        fs::canonicalize(server_directory.join(folder))
            .map(|candidate| candidate == parent)
            .unwrap_or(false)
    });
    if !allowed_parent {
        return Err(String::from(
            "O conteúdo precisa estar diretamente em mods ou plugins deste servidor.",
        ));
    }
    let filename = requested
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| String::from("Nome de arquivo inválido."))?;
    let normalized = filename.to_ascii_lowercase();
    let currently_enabled = normalized.ends_with(".jar");
    let currently_disabled = normalized.ends_with(".jar.disabled");
    if !currently_enabled && !currently_disabled {
        return Err(String::from(
            "O arquivo selecionado não é um JAR gerenciável.",
        ));
    }
    if currently_enabled == enabled {
        return server_content_items(&profile);
    }
    let destination_name = if enabled {
        filename[..filename.len().saturating_sub(".disabled".len())].to_owned()
    } else {
        format!("{filename}.disabled")
    };
    let destination = parent.join(destination_name);
    if destination.exists() {
        return Err(String::from(
            "Já existe um arquivo com o nome de destino; resolva o conflito na pasta primeiro.",
        ));
    }
    fs::rename(&requested, destination).map_err(|error| error.to_string())?;
    server_content_items(&profile)
}

fn toggle_bluemap_files(profile: &ServerProfile, enabled: bool) -> Result<(), String> {
    let items = server_content_items(profile)?;
    for item in items
        .into_iter()
        .filter(|item| item.name.to_ascii_lowercase().contains("bluemap"))
        .filter(|item| item.enabled != enabled)
    {
        let source = PathBuf::from(&item.path);
        let filename = source
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(|| String::from("Nome do JAR do BlueMap inválido."))?;
        let destination_name = if enabled {
            filename
                .strip_suffix(".disabled")
                .ok_or_else(|| String::from("Estado desativado do BlueMap inválido."))?
                .to_owned()
        } else {
            format!("{filename}.disabled")
        };
        let destination = source.with_file_name(destination_name);
        if destination.exists() {
            return Err(String::from(
                "Existe um JAR conflitante do BlueMap; resolva o nome duplicado na pasta primeiro.",
            ));
        }
        fs::rename(&source, &destination)
            .map_err(|error| format!("Não foi possível alterar o estado do BlueMap: {error}"))?;
    }
    Ok(())
}

fn persist_bluemap_pending_state(profile: &ServerProfile, enabled: bool) -> Result<(), String> {
    let path = bluemap_pending_state_path(profile);
    fs::create_dir_all(
        path.parent()
            .ok_or_else(|| String::from("Pasta de estado do BlueMap inválida."))?,
    )
    .map_err(|error| error.to_string())?;
    replace_text_file(&path, &format!("{{\n  \"enabled\": {enabled}\n}}\n"))
}

fn apply_pending_bluemap_state(profile: &ServerProfile) -> Result<(), String> {
    let Some(enabled) = bluemap_pending_enabled(profile) else {
        return Ok(());
    };
    toggle_bluemap_files(profile, enabled)?;
    let _ = fs::remove_file(bluemap_pending_state_path(profile));
    Ok(())
}

#[tauri::command]
fn set_bluemap_enabled(enabled: bool) -> Result<ServerMapStatus, String> {
    let _operation = begin_server_operation()?;
    let profile = read_server_profile();
    let current = server_map_status_for(&profile)?;
    if !current.bluemap_installed {
        return Err(String::from("BlueMap não está instalado neste servidor."));
    }
    if server_status_for_profile(profile.clone()).running {
        persist_bluemap_pending_state(&profile, enabled)?;
    } else {
        toggle_bluemap_files(&profile, enabled)?;
        let _ = fs::remove_file(bluemap_pending_state_path(&profile));
    }
    server_map_status_for(&profile)
}

fn sensitive_pack_path(path: &Path) -> bool {
    path.components().any(|component| {
        let value = component.as_os_str().to_string_lossy().to_ascii_lowercase();
        [
            "secret",
            "token",
            "password",
            "credential",
            "playit",
            "auth",
        ]
        .iter()
        .any(|term| value.contains(term))
    })
}

fn add_safe_config_directory_to_zip(
    writer: &mut zip::ZipWriter<fs::File>,
    root: &Path,
    current: &Path,
    prefix: &str,
    included: &mut usize,
    skipped: &mut usize,
    total_bytes: &mut u64,
) -> Result<(), String> {
    let entries = match fs::read_dir(current) {
        Ok(entries) => entries,
        Err(_) => return Ok(()),
    };
    for entry in entries.flatten() {
        if *included >= 2_000 || *total_bytes >= 128 * 1024 * 1024 {
            *skipped += 1;
            continue;
        }
        let Ok(file_type) = entry.file_type() else {
            *skipped += 1;
            continue;
        };
        if file_type.is_symlink() {
            *skipped += 1;
            continue;
        }
        let path = entry.path();
        if file_type.is_dir() {
            add_safe_config_directory_to_zip(
                writer,
                root,
                &path,
                prefix,
                included,
                skipped,
                total_bytes,
            )?;
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        let relative = match path.strip_prefix(root) {
            Ok(relative) if !sensitive_pack_path(relative) => relative,
            _ => {
                *skipped += 1;
                continue;
            }
        };
        let metadata = fs::metadata(&path).map_err(|error| error.to_string())?;
        if metadata.len() > 16 * 1024 * 1024
            || total_bytes.saturating_add(metadata.len()) > 128 * 1024 * 1024
        {
            *skipped += 1;
            continue;
        }
        let archive_path = format!("{prefix}/{}", relative.to_string_lossy().replace('\\', "/"));
        writer
            .start_file(
                archive_path,
                zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Deflated)
                    .unix_permissions(0o644),
            )
            .map_err(|error| error.to_string())?;
        let mut source = fs::File::open(&path).map_err(|error| error.to_string())?;
        std::io::copy(&mut source, writer).map_err(|error| error.to_string())?;
        *included += 1;
        *total_bytes = total_bytes.saturating_add(metadata.len());
    }
    Ok(())
}

#[tauri::command]
fn export_server_client_pack(
    include_unknown: bool,
    include_configs: bool,
) -> Result<ServerPackExportResult, String> {
    let profile = read_server_profile();
    let directory = current_server_directory(&profile)?;
    let items = server_content_items(&profile)?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default();
    let export_directory = storage_root().join("exports").join("server-packs");
    fs::create_dir_all(&export_directory).map_err(|error| error.to_string())?;
    let destination = export_directory.join(format!(
        "{}-client-pack-{timestamp}.zip",
        safe_directory_name(&profile.name)
    ));
    let file = fs::File::create(&destination).map_err(|error| error.to_string())?;
    let mut writer = zip::ZipWriter::new(file);
    let text_options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o644);
    let binary_options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored)
        .unix_permissions(0o644);
    let mut included_files = 0_usize;
    let mut skipped_files = 0_usize;
    let mut warnings = Vec::new();
    let mut included_mods = Vec::new();

    for item in items.iter().filter(|item| item.kind == "mod") {
        if !item.enabled
            || item.side == "server-only"
            || (item.side == "unknown" && !include_unknown)
        {
            skipped_files += 1;
            continue;
        }
        let source_path = PathBuf::from(&item.path);
        let archive_name = format!("mods/{}", item.name);
        writer
            .start_file(archive_name, binary_options)
            .map_err(|error| error.to_string())?;
        let mut source = fs::File::open(source_path).map_err(|error| error.to_string())?;
        std::io::copy(&mut source, &mut writer).map_err(|error| error.to_string())?;
        included_files += 1;
        included_mods.push(serde_json::json!({
            "name": item.name,
            "side": item.side,
            "size_mb": item.size_mb
        }));
    }
    if items
        .iter()
        .any(|item| item.kind == "mod" && item.side == "unknown")
    {
        warnings.push(String::from(
            "Alguns mods não declaram o lado com clareza. Revise a lista antes de enviar o ZIP.",
        ));
    }

    if include_configs {
        let mut config_bytes = 0_u64;
        for folder in ["config", "defaultconfigs"] {
            let source = directory.join(folder);
            if source.is_dir() {
                add_safe_config_directory_to_zip(
                    &mut writer,
                    &source,
                    &source,
                    folder,
                    &mut included_files,
                    &mut skipped_files,
                    &mut config_bytes,
                )?;
            }
        }
        warnings.push(String::from(
            "Configs sensíveis, links simbólicos e arquivos grandes foram excluídos automaticamente.",
        ));
    }

    let manifest = serde_json::json!({
        "format": 1,
        "created_by": "VEX Launcher",
        "server": profile.name,
        "minecraft": profile.version,
        "loader": profile.software,
        "mods": included_mods,
        "notes": "ZIP genérico para copiar sobre uma instância compatível. O VEX não inclui Java nem credenciais."
    });
    writer
        .start_file("vex-pack.json", text_options)
        .map_err(|error| error.to_string())?;
    writer
        .write_all(
            serde_json::to_string_pretty(&manifest)
                .map_err(|error| error.to_string())?
                .as_bytes(),
        )
        .map_err(|error| error.to_string())?;
    included_files += 1;
    writer
        .start_file("LEIA-ME.txt", text_options)
        .map_err(|error| error.to_string())?;
    writer
        .write_all(
            format!(
                "Pack de cliente do servidor {}\nMinecraft {} · {}\n\n1. Crie uma instância com a mesma versão e loader.\n2. Extraia este ZIP na pasta da instância.\n3. Revise os mods marcados como desconhecidos no vex-pack.json.\n4. Nenhuma senha, token, mundo ou plugin de servidor foi incluído.\n",
                profile.name, profile.version, profile.software
            )
            .as_bytes(),
        )
        .map_err(|error| error.to_string())?;
    included_files += 1;
    writer.finish().map_err(|error| error.to_string())?;
    Ok(ServerPackExportResult {
        path: destination.to_string_lossy().into_owned(),
        included_files,
        skipped_files,
        warnings,
    })
}

fn copy_safe_config_directory_to_instance(
    root: &Path,
    current: &Path,
    destination_root: &Path,
    included: &mut usize,
    skipped: &mut usize,
    total_bytes: &mut u64,
) -> Result<(), String> {
    let entries = match fs::read_dir(current) {
        Ok(entries) => entries,
        Err(_) => return Ok(()),
    };
    for entry in entries.flatten() {
        if *included >= 2_000 || *total_bytes >= 128 * 1024 * 1024 {
            *skipped += 1;
            continue;
        }
        let Ok(file_type) = entry.file_type() else {
            *skipped += 1;
            continue;
        };
        if file_type.is_symlink() {
            *skipped += 1;
            continue;
        }
        let path = entry.path();
        if file_type.is_dir() {
            copy_safe_config_directory_to_instance(
                root,
                &path,
                destination_root,
                included,
                skipped,
                total_bytes,
            )?;
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        let relative = match path.strip_prefix(root) {
            Ok(relative) if !sensitive_pack_path(relative) => relative,
            _ => {
                *skipped += 1;
                continue;
            }
        };
        let metadata = fs::metadata(&path).map_err(|error| error.to_string())?;
        if metadata.len() > 16 * 1024 * 1024
            || total_bytes.saturating_add(metadata.len()) > 128 * 1024 * 1024
        {
            *skipped += 1;
            continue;
        }
        let destination = destination_root.join(relative);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        fs::copy(&path, destination).map_err(|error| error.to_string())?;
        *included += 1;
        *total_bytes = total_bytes.saturating_add(metadata.len());
    }
    Ok(())
}

fn copy_server_client_payload(
    profile: &ServerProfile,
    server_directory: &Path,
    instance_directory: &Path,
    include_unknown: bool,
    include_configs: bool,
) -> Result<(usize, usize, Vec<String>), String> {
    let items = server_content_items(profile)?;
    let mods_directory = instance_directory.join("mods");
    fs::create_dir_all(&mods_directory).map_err(|error| error.to_string())?;
    let mut included = 0_usize;
    let mut skipped = 0_usize;
    let mut total_mod_bytes = 0_u64;
    let mut warnings = Vec::new();
    let has_unknown = items
        .iter()
        .any(|item| item.kind == "mod" && item.side == "unknown");
    for item in items.iter().filter(|item| item.kind == "mod") {
        if !item.enabled
            || item.side == "server-only"
            || (item.side == "unknown" && !include_unknown)
        {
            skipped += 1;
            continue;
        }
        let source = PathBuf::from(&item.path);
        let metadata = fs::symlink_metadata(&source).map_err(|error| error.to_string())?;
        if !metadata.is_file()
            || metadata.file_type().is_symlink()
            || metadata.len() > 512 * 1024 * 1024
            || total_mod_bytes.saturating_add(metadata.len()) > 4 * 1024 * 1024 * 1024
        {
            skipped += 1;
            continue;
        }
        fs::copy(&source, mods_directory.join(&item.name))
            .map_err(|error| format!("Não foi possível copiar {}: {error}", item.name))?;
        included += 1;
        total_mod_bytes = total_mod_bytes.saturating_add(metadata.len());
    }
    if has_unknown {
        warnings.push(if include_unknown {
            String::from(
                "Mods sem declaração de lado foram incluídos. Se o cliente não abrir, revise esses arquivos.",
            )
        } else {
            String::from("Mods sem declaração de lado foram excluídos da instância.")
        });
    }
    if include_configs {
        let mut config_bytes = 0_u64;
        for folder in ["config", "defaultconfigs"] {
            let source = server_directory.join(folder);
            if source.is_dir() {
                copy_safe_config_directory_to_instance(
                    &source,
                    &source,
                    &instance_directory.join(folder),
                    &mut included,
                    &mut skipped,
                    &mut config_bytes,
                )?;
            }
        }
        warnings.push(String::from(
            "Configurações sensíveis, links simbólicos e arquivos grandes foram excluídos automaticamente.",
        ));
    }
    Ok((included, skipped, warnings))
}

fn push_nbt_name(buffer: &mut Vec<u8>, value: &str) -> Result<(), String> {
    let length = u16::try_from(value.len()).map_err(|_| String::from("Texto NBT muito grande."))?;
    buffer.extend_from_slice(&length.to_be_bytes());
    buffer.extend_from_slice(value.as_bytes());
    Ok(())
}

fn push_nbt_string_tag(buffer: &mut Vec<u8>, name: &str, value: &str) -> Result<(), String> {
    buffer.push(8);
    push_nbt_name(buffer, name)?;
    push_nbt_name(buffer, value)
}

fn write_local_server_entry(
    instance_directory: &Path,
    name: &str,
    address: &str,
) -> Result<(), String> {
    let mut nbt = Vec::new();
    nbt.extend_from_slice(&[10, 0, 0]);
    nbt.push(9);
    push_nbt_name(&mut nbt, "servers")?;
    nbt.push(10);
    nbt.extend_from_slice(&1_i32.to_be_bytes());
    push_nbt_string_tag(&mut nbt, "name", name)?;
    push_nbt_string_tag(&mut nbt, "ip", address)?;
    nbt.push(1);
    push_nbt_name(&mut nbt, "hidden")?;
    nbt.push(0);
    nbt.push(0);
    nbt.push(0);
    fs::write(instance_directory.join("servers.dat"), nbt).map_err(|error| error.to_string())
}

fn parse_server_address(value: &str) -> Option<(String, u16)> {
    let (host, port) = value.trim().rsplit_once(':')?;
    let host = host.trim();
    let port = port.parse::<u16>().ok()?;
    if host.is_empty()
        || host.len() > 253
        || port == 0
        || !host
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '.' | '-'))
    {
        return None;
    }
    Some((host.to_owned(), port))
}

fn client_loader_for_server(profile: &ServerProfile, directory: &Path) -> Result<String, String> {
    let software = if matches!(profile.software.as_str(), "modded" | "unknown") {
        detected_server_software(directory, resolved_server_target(profile).ok().as_deref())
    } else {
        profile.software.clone()
    };
    match software.as_str() {
        "vanilla" | "paper" | "purpur" | "spigot" | "bukkit" => Ok(String::from("vanilla")),
        "fabric" | "quilt" | "forge" | "neoforge" => Ok(software),
        _ => Err(String::from(
            "O VEX não identificou o loader do servidor com segurança. Encontre a pasta novamente ou confira o script de início.",
        )),
    }
}

#[tauri::command]
async fn create_server_playable_instance(
    app: tauri::AppHandle,
    include_unknown: bool,
    include_configs: bool,
) -> Result<ServerPlayableInstanceResult, String> {
    let _operation = begin_server_operation()?;
    let registry = read_server_registry();
    let profile = registry
        .servers
        .iter()
        .find(|profile| profile.id == registry.active_id)
        .cloned()
        .ok_or_else(|| String::from("Adicione um servidor antes de criar a instância."))?;
    let server_directory = current_server_directory(&profile)?;
    let loader = client_loader_for_server(&profile, &server_directory)?;
    if profile.version.trim().is_empty() || profile.version == "desconhecida" {
        return Err(String::from(
            "A versão do Minecraft não foi identificada. Inicie o servidor uma vez e encontre a pasta novamente.",
        ));
    }
    let operation = "create-server-instance";
    emit_progress(
        &app,
        operation,
        "Preparando instância do servidor",
        5,
        false,
    );
    let settings = read_settings();
    let game_dir = PathBuf::from(&settings.game_directory);
    let instances_root = storage_root().join("instances");
    fs::create_dir_all(&instances_root).map_err(|error| error.to_string())?;
    let id_suffix: String = profile
        .id
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .take(16)
        .collect();
    let directory_name = safe_directory_name(&format!("{}-cliente-{id_suffix}", profile.name));
    let destination = instances_root.join(&directory_name);
    if destination.exists() {
        return Err(String::from(
            "Esta instância do servidor já existe. Abra a Biblioteca para jogar ou apague a instância antes de recriá-la.",
        ));
    }
    let version_id =
        resolve_instance_version_id(&app, &game_dir, &profile.version, &loader, operation).await?;
    emit_progress(&app, operation, "Copiando conteúdo necessário", 76, false);
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let staging = instances_root.join(format!(".{directory_name}.part-{nonce}"));
    fs::create_dir_all(&staging).map_err(|error| error.to_string())?;
    let payload = copy_server_client_payload(
        &profile,
        &server_directory,
        &staging,
        include_unknown,
        include_configs,
    );
    let (included_files, skipped_files, warnings) = match payload {
        Ok(result) => result,
        Err(error) => {
            let _ = fs::remove_dir_all(&staging);
            return Err(error);
        }
    };
    let server_address = format!("localhost:{}", profile.port);
    if let Err(error) = write_local_server_entry(&staging, &profile.name, &server_address) {
        let _ = fs::remove_dir_all(&staging);
        return Err(error);
    }
    let instance_name = format!("{} · Cliente", profile.name);
    let instance_id = format!("server-client-{}", profile.id);
    let metadata = serde_json::json!({
        "Id": instance_id,
        "Name": instance_name,
        "McVersion": profile.version,
        "Loader": loader,
        "VersionId": version_id,
        "Source": "VEX Server Manager",
        "Kind": "server-instance",
        "ServerProfileId": profile.id,
        "ServerName": profile.name,
        "ServerAddress": server_address,
        "InstallDate": "created-by-vex-server-manager"
    });
    if let Err(error) = fs::write(
        staging.join("instance.json"),
        serde_json::to_string_pretty(&metadata).map_err(|error| error.to_string())?,
    ) {
        let _ = fs::remove_dir_all(&staging);
        return Err(error.to_string());
    }
    if let Err(error) = fs::rename(&staging, &destination) {
        let _ = fs::remove_dir_all(&staging);
        return Err(format!(
            "Não foi possível concluir a instância no HD: {error}"
        ));
    }
    emit_progress(&app, operation, "Instância pronta para jogar", 100, true);
    let instance = InstalledInstance {
        id: metadata["Id"].as_str().unwrap_or_default().to_owned(),
        name: metadata["Name"].as_str().unwrap_or_default().to_owned(),
        loader: metadata["Loader"].as_str().unwrap_or_default().to_owned(),
        mc_version: metadata["McVersion"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
        version_id: metadata["VersionId"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
        profile_dir: destination.to_string_lossy().into_owned(),
        icon_path: None,
        kind: String::from("server-instance"),
        size_mb: (directory_size(&destination) as f64 / 1_048_576.0 * 10.0).round() / 10.0,
        modified_unix: modified_unix(&destination),
        last_played_unix: 0,
    };
    Ok(ServerPlayableInstanceResult {
        instance,
        included_files,
        skipped_files,
        warnings,
        server_address,
    })
}

fn configured_port(directory: &Path, candidates: &[&str], keys: &[&str]) -> Option<u16> {
    for candidate in candidates {
        let Some(content) = read_small_text(&directory.join(candidate), 2 * 1024 * 1024) else {
            continue;
        };
        for line in content.lines() {
            let clean = line.trim();
            if clean.starts_with('#') || clean.starts_with("//") {
                continue;
            }
            for key in keys {
                let Some(value) = clean.strip_prefix(key) else {
                    continue;
                };
                let digits: String = value
                    .chars()
                    .skip_while(|character| !character.is_ascii_digit())
                    .take_while(|character| character.is_ascii_digit())
                    .collect();
                if let Ok(port) = digits.parse::<u16>() {
                    if port > 0 {
                        return Some(port);
                    }
                }
            }
        }
    }
    None
}

fn config_boolean(directory: &Path, candidates: &[&str], key: &str) -> Option<bool> {
    for candidate in candidates {
        let Some(content) = read_small_text(&directory.join(candidate), 2 * 1024 * 1024) else {
            continue;
        };
        for line in content.lines() {
            let clean = line.trim();
            if clean.starts_with('#') || clean.starts_with("//") {
                continue;
            }
            let Some((name, value)) = clean.split_once([':', '=']) else {
                continue;
            };
            if name.trim() == key {
                return match value
                    .trim()
                    .trim_end_matches(',')
                    .to_ascii_lowercase()
                    .as_str()
                {
                    "true" => Some(true),
                    "false" => Some(false),
                    _ => None,
                };
            }
        }
    }
    None
}

#[cfg(target_os = "linux")]
fn available_disk_gb(path: &Path) -> f64 {
    hidden_command("df")
        .arg("-Pk")
        .arg("--")
        .arg(path)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .and_then(|content| {
            content
                .lines()
                .rfind(|line| !line.trim().is_empty())
                .and_then(|line| line.split_whitespace().nth(3))
                .and_then(|value| value.parse::<u64>().ok())
        })
        .map(|kilobytes| kilobytes as f64 / 1024.0 / 1024.0)
        .unwrap_or_default()
}

#[cfg(windows)]
fn available_disk_gb(path: &Path) -> f64 {
    hidden_command("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "$root = [System.IO.Path]::GetPathRoot($args[0]); ([System.IO.DriveInfo]::new($root)).AvailableFreeSpace",
        ])
        .arg(path)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(|bytes| bytes as f64 / 1024.0 / 1024.0 / 1024.0)
        .unwrap_or_default()
}

#[cfg(all(not(target_os = "linux"), not(windows)))]
fn available_disk_gb(_path: &Path) -> f64 {
    0.0
}

#[cfg(target_os = "linux")]
fn same_storage_device(left: &Path, right: &Path) -> bool {
    match (fs::metadata(left), fs::metadata(right)) {
        (Ok(left), Ok(right)) => left.dev() == right.dev(),
        _ => false,
    }
}

#[cfg(windows)]
fn same_storage_device(left: &Path, right: &Path) -> bool {
    let left = fs::canonicalize(left).unwrap_or_else(|_| left.to_path_buf());
    let right = fs::canonicalize(right).unwrap_or_else(|_| right.to_path_buf());
    left.components().next() == right.components().next()
}

#[cfg(all(not(windows), not(target_os = "linux")))]
fn same_storage_device(_left: &Path, _right: &Path) -> bool {
    false
}

fn server_maps_root(profile: &ServerProfile) -> Result<PathBuf, String> {
    let root = storage_root()
        .join("maps")
        .join(safe_server_profile_id(profile));
    fs::create_dir_all(&root).map_err(|error| {
        format!(
            "Não foi possível preparar a pasta padrão de mapas {}: {error}",
            root.display()
        )
    })?;
    fs::canonicalize(&root).map_err(|error| error.to_string())
}

fn bluemap_pending_state_path(profile: &ServerProfile) -> PathBuf {
    server_runtime_directory(profile).join("bluemap-enabled.json")
}

fn bluemap_pending_enabled(profile: &ServerProfile) -> Option<bool> {
    fs::read_to_string(bluemap_pending_state_path(profile))
        .ok()
        .and_then(|value| serde_json::from_str::<Value>(&value).ok())
        .and_then(|value| value.get("enabled").and_then(Value::as_bool))
}

fn configured_bluemap_threads(directory: &Path) -> u32 {
    [
        directory.join("config/bluemap/core.conf"),
        directory.join("plugins/BlueMap/core.conf"),
        directory.join("bluemap/core.conf"),
    ]
    .iter()
    .filter_map(|path| fs::read_to_string(path).ok())
    .find_map(|content| {
        content.lines().find_map(|line| {
            let clean = line.trim();
            let (key, value) = clean.split_once([':', '='])?;
            (key.trim() == "render-thread-count")
                .then(|| value.trim().parse::<u32>().ok())
                .flatten()
        })
    })
    .unwrap_or(1)
}

fn server_map_status_for(profile: &ServerProfile) -> Result<ServerMapStatus, String> {
    let directory = current_server_directory(profile)?;
    let items = server_content_items(profile)?;
    let enabled_names: Vec<String> = items
        .iter()
        .filter(|item| item.enabled)
        .map(|item| item.name.to_ascii_lowercase())
        .collect();
    let has = |needle: &str| enabled_names.iter().any(|name| name.contains(needle));
    let bluemap_installed = items
        .iter()
        .any(|item| item.name.to_ascii_lowercase().contains("bluemap"));
    let bluemap_enabled = has("bluemap");
    let providers = if bluemap_enabled {
        vec![String::from("bluemap")]
    } else {
        Vec::new()
    };
    let bluemap_port = bluemap_enabled.then(|| {
        configured_port(
            &directory,
            &[
                "bluemap/webserver.conf",
                "config/bluemap/webserver.conf",
                "plugins/BlueMap/webserver.conf",
            ],
            &["port"],
        )
        .unwrap_or(8100)
    });
    let provider = bluemap_enabled.then(|| String::from("bluemap"));
    let port = bluemap_port;
    let web_url = port.map(|port| format!("http://127.0.0.1:{port}"));
    let bluemap_web_url = bluemap_port.map(|port| format!("http://127.0.0.1:{port}"));
    let port_online = |port: u16| {
        TcpStream::connect_timeout(
            &SocketAddr::from(([127, 0, 0, 1], port)),
            Duration::from_millis(180),
        )
        .is_ok()
    };
    let maps_root = server_maps_root(profile).ok();
    let bluemap_online = bluemap_port.is_some_and(port_online);
    let online = port.is_some_and(port_online);
    let properties = read_server_properties(&directory);
    let world_name = properties
        .get("level-name")
        .filter(|value| !value.trim().is_empty())
        .cloned()
        .unwrap_or_else(|| String::from("world"));
    let world_candidate = directory.join(&world_name);
    let world_directory = fs::canonicalize(&world_candidate).unwrap_or(world_candidate);
    let heavy_storage = maps_root;
    let map_available_disk_gb = heavy_storage
        .as_deref()
        .map(available_disk_gb)
        .unwrap_or_default();
    let (bluemap_storage_safe, bluemap_storage_message) = if map_available_disk_gb < 5.0 {
        (
            false,
            String::from("Há menos de 5 GB livres no disco padrão do VEX para o BlueMap."),
        )
    } else {
        (
            true,
            format!(
                "BlueMap usará automaticamente a pasta de mapas do VEX, com {map_available_disk_gb:.1} GB livres."
            ),
        )
    };
    let bluemap_configured = bluemap_enabled
        && config_boolean(
            &directory,
            &[
                "config/bluemap/core.conf",
                "plugins/BlueMap/core.conf",
                "bluemap/core.conf",
            ],
            "accept-download",
        ) == Some(true);
    let (storage_safe, storage_message) = if map_available_disk_gb < 5.0 {
        (
            false,
            String::from("Há menos de 5 GB livres na pasta padrão de mapas do VEX."),
        )
    } else {
        (
            true,
            format!("Mapas usarão o HD configurado, com {map_available_disk_gb:.1} GB livres."),
        )
    };
    let world_available_disk_gb = available_disk_gb(&world_directory);
    let (world_generation_safe, world_generation_message) = if !world_directory.is_dir() {
        (
            false,
            String::from("O mundo ainda não existe; inicie o servidor uma vez para criá-lo."),
        )
    } else if !same_storage_device(
        &world_directory,
        heavy_storage.as_deref().unwrap_or(&directory),
    ) {
        (
            false,
            String::from(
                "O mundo está em outro disco. O VEX bloqueou a geração para não encher o SSD.",
            ),
        )
    } else if world_available_disk_gb < 5.0 {
        (
            false,
            String::from("Há menos de 5 GB livres no disco onde o mundo está armazenado."),
        )
    } else {
        (
            true,
            format!("Disco do mundo validado com {world_available_disk_gb:.1} GB livres."),
        )
    };
    Ok(ServerMapStatus {
        provider,
        providers,
        web_url,
        bluemap_web_url,
        bluemap_online,
        port,
        online,
        chunky_installed: has("chunky"),
        bluemap_installed,
        bluemap_enabled,
        bluemap_pending_enabled: bluemap_pending_enabled(profile),
        bluemap_render_threads: configured_bluemap_threads(&directory),
        bluemap_max_threads: std::thread::available_parallelism()
            .map(|value| value.get() as u32)
            .unwrap_or(1),
        bluemap_configured,
        bluemap_storage_safe,
        bluemap_storage_message,
        world_name,
        world_directory: world_directory.to_string_lossy().into_owned(),
        heavy_storage_directory: heavy_storage.map(|path| path.to_string_lossy().into_owned()),
        storage_safe,
        storage_message,
        world_generation_safe,
        world_generation_message,
        world_available_disk_gb,
        available_disk_gb: map_available_disk_gb,
    })
}

#[tauri::command]
fn server_map_status() -> Result<ServerMapStatus, String> {
    server_map_status_for(&read_server_profile())
}

#[tauri::command]
async fn server_last_player_location(name: Option<String>) -> Result<ServerMapPlayer, String> {
    let profile = read_server_profile();
    let directory = current_server_directory(&profile)?;
    let world_name = read_server_properties(&directory)
        .get("level-name")
        .filter(|value| !value.trim().is_empty())
        .cloned()
        .unwrap_or_else(|| String::from("world"));
    let cache: HashMap<String, String> = fs::read_to_string(directory.join("usercache.json"))
        .ok()
        .and_then(|content| serde_json::from_str::<Value>(&content).ok())
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|entry| {
            Some((
                entry.get("uuid")?.as_str()?.to_ascii_lowercase(),
                entry.get("name")?.as_str()?.to_owned(),
            ))
        })
        .collect();
    let requested = name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let playerdata = directory.join(&world_name).join("playerdata");
    let mut files: Vec<PathBuf> = fs::read_dir(&playerdata)
        .map_err(|_| String::from("O mundo ainda não possui fichas de jogadores."))?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("dat"))
        .collect();
    files.sort_by_key(|path| {
        std::cmp::Reverse(
            fs::metadata(path)
                .and_then(|metadata| metadata.modified())
                .unwrap_or(UNIX_EPOCH),
        )
    });
    for path in files.into_iter().take(1_000) {
        let uuid = path
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let player_name = cache.get(&uuid).cloned().unwrap_or_else(|| uuid.clone());
        if requested.is_some_and(|name| !player_name.eq_ignore_ascii_case(name)) {
            continue;
        }
        if let Some((position, dimension)) = read_player_nbt_position(&path) {
            let world = match dimension.as_str() {
                "minecraft:the_nether" => format!("{world_name}_nether"),
                "minecraft:the_end" => format!("{world_name}_the_end"),
                _ => world_name.clone(),
            };
            return Ok(ServerMapPlayer {
                name: player_name,
                uuid,
                world,
                x: Some(position[0]),
                y: Some(position[1]),
                z: Some(position[2]),
                yaw: None,
                health: None,
                armor: None,
            });
        }
    }
    Err(match requested {
        Some(name) => format!("Não há uma posição salva para {name}."),
        None => String::from("Nenhum jogador com posição válida foi encontrado no mundo."),
    })
}

fn read_nbt_u8(cursor: &mut Cursor<Vec<u8>>) -> Option<u8> {
    let mut bytes = [0_u8; 1];
    cursor.read_exact(&mut bytes).ok()?;
    Some(bytes[0])
}

fn read_nbt_i32(cursor: &mut Cursor<Vec<u8>>) -> Option<i32> {
    let mut bytes = [0_u8; 4];
    cursor.read_exact(&mut bytes).ok()?;
    Some(i32::from_be_bytes(bytes))
}

fn read_nbt_string(cursor: &mut Cursor<Vec<u8>>) -> Option<String> {
    let mut length = [0_u8; 2];
    cursor.read_exact(&mut length).ok()?;
    let length = u16::from_be_bytes(length) as usize;
    if length > 65_535 {
        return None;
    }
    let mut bytes = vec![0_u8; length];
    cursor.read_exact(&mut bytes).ok()?;
    String::from_utf8(bytes).ok()
}

fn skip_nbt_payload(cursor: &mut Cursor<Vec<u8>>, tag: u8, depth: u8) -> Option<()> {
    if depth > 32 {
        return None;
    }
    let skip = |cursor: &mut Cursor<Vec<u8>>, bytes: u64| {
        let next = cursor.position().checked_add(bytes)?;
        (next <= cursor.get_ref().len() as u64).then(|| cursor.set_position(next))
    };
    match tag {
        1 => skip(cursor, 1),
        2 => skip(cursor, 2),
        3 | 5 => skip(cursor, 4),
        4 | 6 => skip(cursor, 8),
        7 => {
            let length = read_nbt_i32(cursor)?;
            (length >= 0).then_some(())?;
            skip(cursor, length as u64)
        }
        8 => read_nbt_string(cursor).map(|_| ()),
        9 => {
            let child = read_nbt_u8(cursor)?;
            let length = read_nbt_i32(cursor)?;
            if !(0..=2_000_000).contains(&length) {
                return None;
            }
            for _ in 0..length {
                skip_nbt_payload(cursor, child, depth + 1)?;
            }
            Some(())
        }
        10 => {
            loop {
                let child = read_nbt_u8(cursor)?;
                if child == 0 {
                    break;
                }
                read_nbt_string(cursor)?;
                skip_nbt_payload(cursor, child, depth + 1)?;
            }
            Some(())
        }
        11 => {
            let length = read_nbt_i32(cursor)?;
            (0..=2_000_000).contains(&length).then_some(())?;
            skip(cursor, length as u64 * 4)
        }
        12 => {
            let length = read_nbt_i32(cursor)?;
            (0..=2_000_000).contains(&length).then_some(())?;
            skip(cursor, length as u64 * 8)
        }
        _ => None,
    }
}

fn read_player_nbt_position(path: &Path) -> Option<([f64; 3], String)> {
    let file = fs::File::open(path).ok()?;
    let mut decoder = flate2::read::GzDecoder::new(file).take(2 * 1024 * 1024);
    let mut bytes = Vec::new();
    decoder.read_to_end(&mut bytes).ok()?;
    let mut cursor = Cursor::new(bytes);
    (read_nbt_u8(&mut cursor)? == 10).then_some(())?;
    read_nbt_string(&mut cursor)?;
    let mut position = None;
    let mut dimension = String::from("minecraft:overworld");
    loop {
        let tag = read_nbt_u8(&mut cursor)?;
        if tag == 0 {
            break;
        }
        let key = read_nbt_string(&mut cursor)?;
        if key == "Pos" && tag == 9 {
            let child = read_nbt_u8(&mut cursor)?;
            let length = read_nbt_i32(&mut cursor)?;
            if child != 6 || length != 3 {
                return None;
            }
            let mut values = [0.0_f64; 3];
            for value in &mut values {
                let mut number = [0_u8; 8];
                cursor.read_exact(&mut number).ok()?;
                *value = f64::from_be_bytes(number);
            }
            position = values
                .iter()
                .all(|value| value.is_finite())
                .then_some(values);
        } else if key == "Dimension" && tag == 8 {
            dimension = read_nbt_string(&mut cursor)?;
        } else {
            skip_nbt_payload(&mut cursor, tag, 0)?;
        }
    }
    position.map(|position| (position, dimension))
}

#[tauri::command]
fn set_server_heavy_storage(directory: Option<String>) -> Result<ServerProfile, String> {
    let _operation = begin_server_operation()?;
    ensure_server_stopped_for_profile_change()?;
    let mut profile = read_server_profile();
    profile.heavy_storage_directory = directory;
    save_server_profile_internal(profile)
}

fn validated_server_identifier(value: &str, label: &str) -> Result<String, String> {
    let clean = value.trim();
    if clean.is_empty()
        || clean.len() > 128
        || !clean.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.' | ':')
        })
    {
        return Err(format!("{label} contém caracteres não permitidos."));
    }
    Ok(clean.to_owned())
}

fn server_map_task_path(profile: &ServerProfile) -> PathBuf {
    server_runtime_directory(profile).join("map-task.json")
}

fn map_native_log_path(profile: &ServerProfile) -> PathBuf {
    PathBuf::from(&profile.directory)
        .join("logs")
        .join("latest.log")
}

fn file_length(path: &Path) -> u64 {
    fs::metadata(path)
        .map(|metadata| metadata.len())
        .unwrap_or_default()
}

fn start_map_task(profile: &ServerProfile, operation: &str, provider: &str, label: &str) {
    let record = ServerMapTaskRecord {
        operation: operation.to_owned(),
        provider: provider.to_owned(),
        label: label.to_owned(),
        started_unix: unix_seconds(),
        active: true,
        native_log_offset: file_length(&map_native_log_path(profile)),
        vex_log_offset: file_length(&server_log_path_for(profile)),
        last_probe_unix: 0,
        auto_stop: provider.eq_ignore_ascii_case("bluemap"),
    };
    if let Ok(content) = serde_json::to_string_pretty(&record) {
        let _ = replace_text_file(&server_map_task_path(profile), &content);
    }
}

fn update_map_task_active(profile: &ServerProfile, active: bool) {
    let path = server_map_task_path(profile);
    let Some(mut record) = fs::read_to_string(&path)
        .ok()
        .and_then(|content| serde_json::from_str::<ServerMapTaskRecord>(&content).ok())
    else {
        return;
    };
    record.active = active;
    if let Ok(content) = serde_json::to_string_pretty(&record) {
        let _ = replace_text_file(&path, &content);
    }
}

fn read_log_after(path: &Path, offset: u64) -> String {
    const MAX_TASK_LOG_BYTES: u64 = 2 * 1024 * 1024;
    let Ok(mut file) = fs::File::open(path) else {
        return String::new();
    };
    let length = file
        .metadata()
        .map(|metadata| metadata.len())
        .unwrap_or_default();
    let start = if length >= offset {
        offset.max(length.saturating_sub(MAX_TASK_LOG_BYTES))
    } else {
        length.saturating_sub(MAX_TASK_LOG_BYTES)
    };
    if file.seek(SeekFrom::Start(start)).is_err() {
        return String::new();
    }
    let mut content = String::new();
    let _ = file.take(MAX_TASK_LOG_BYTES).read_to_string(&mut content);
    content
}

fn percentage_in_line(line: &str) -> Option<f32> {
    let percent_index = line.rfind('%')?;
    let prefix = &line[..percent_index];
    let start = prefix
        .char_indices()
        .rev()
        .find(|(_, character)| {
            !character.is_ascii_digit() && *character != '.' && *character != ','
        })
        .map(|(index, character)| index + character.len_utf8())
        .unwrap_or_default();
    prefix[start..]
        .trim()
        .replace(',', ".")
        .parse::<f32>()
        .ok()
        .filter(|value| value.is_finite() && (0.0..=100.0).contains(value))
}

#[tauri::command]
fn server_map_task_progress() -> ServerMapTaskProgress {
    let profile = read_server_profile();
    let path = server_map_task_path(&profile);
    let Some(mut record) = fs::read_to_string(&path)
        .ok()
        .and_then(|content| serde_json::from_str::<ServerMapTaskRecord>(&content).ok())
    else {
        return ServerMapTaskProgress {
            operation: String::from("idle"),
            provider: String::new(),
            active: false,
            determinate: false,
            percent: None,
            label: String::from("Nenhuma tarefa de mapa em andamento"),
            detail: String::from("Escolha uma região para renderizar ou gerar."),
            eta_seconds: None,
            started_unix: None,
        };
    };
    let server_running = server_status_for_profile(profile.clone()).running;
    if !server_running {
        record.active = false;
    }
    if record.active
        && record.provider.eq_ignore_ascii_case("bluemap")
        && unix_seconds().saturating_sub(record.last_probe_unix) >= 4
    {
        let _ = send_server_command_for_profile(&profile, String::from("bluemap"));
        record.last_probe_unix = unix_seconds();
        if let Ok(content) = serde_json::to_string_pretty(&record) {
            let _ = replace_text_file(&path, &content);
        }
    }
    let logs = format!(
        "{}\n{}",
        read_log_after(&map_native_log_path(&profile), record.native_log_offset),
        read_log_after(&server_log_path_for(&profile), record.vex_log_offset)
    );
    let provider_lower = record.provider.to_ascii_lowercase();
    let mut percent = None;
    let mut last_relevant = String::new();
    let mut completed = false;
    let mut bluemap_status_block = false;
    let mut native_eta_seconds = None;
    for line in logs.lines() {
        let lower = line.to_ascii_lowercase();
        if record.provider.eq_ignore_ascii_case("bluemap") {
            if lower.contains("bluemap status") {
                bluemap_status_block = true;
            } else if bluemap_status_block
                && (lower.contains("progress:")
                    || lower.contains("remaining time:")
                    || lower.contains("currently being updated")
                    || lower.contains("pending updates")
                    || lower.contains("render-thread"))
            {
                last_relevant = line.chars().take(240).collect();
                if let Some(value) = percentage_in_line(line) {
                    percent = Some(value);
                }
                if let Some(value) = lower.split("remaining time:").nth(1) {
                    let amount = value
                        .split_whitespace()
                        .next()
                        .and_then(|value| value.replace(',', ".").parse::<f64>().ok());
                    let factor = if value.contains("day") || value.contains("dia") {
                        86_400.0
                    } else if value.contains("hour") || value.contains("hora") {
                        3_600.0
                    } else if value.contains("minute") || value.contains("minuto") {
                        60.0
                    } else {
                        1.0
                    };
                    native_eta_seconds = amount.map(|amount| (amount * factor) as u64);
                }
                continue;
            } else if bluemap_status_block && line.trim().is_empty() {
                bluemap_status_block = false;
            }
        }
        let relevant = match provider_lower.as_str() {
            "bluemap" => lower.contains("bluemap") || lower.contains("updating map"),
            "chunky" => lower.contains("chunky") || lower.contains("chunk generation"),
            _ => false,
        };
        if !relevant {
            continue;
        }
        last_relevant = line.chars().take(240).collect();
        if let Some(value) = percentage_in_line(line) {
            percent = Some(value);
        }
        if (lower.contains("finished")
            || lower.contains("completed")
            || lower.contains("complete")
            || lower.contains("rendered"))
            && !lower.contains("not complete")
        {
            completed = true;
        }
    }
    if percent.is_some_and(|value| value >= 100.0) || completed {
        record.active = false;
        percent = Some(100.0);
        if record.auto_stop && server_running {
            let _ = send_server_command_for_profile(&profile, String::from("bluemap stop"));
            record.auto_stop = false;
        }
    }
    if !record.active {
        update_map_task_active(&profile, false);
    }
    let elapsed = unix_seconds().saturating_sub(record.started_unix);
    let eta_seconds = native_eta_seconds.or_else(|| {
        percent.and_then(|value| {
            (record.active && (1.0..100.0).contains(&value))
                .then(|| ((elapsed as f64 * (100.0 - f64::from(value))) / f64::from(value)) as u64)
        })
    });
    ServerMapTaskProgress {
        operation: record.operation,
        provider: record.provider,
        active: record.active,
        determinate: percent.is_some(),
        percent,
        label: if record.active {
            record.label
        } else if completed || percent == Some(100.0) {
            String::from("Tarefa concluída")
        } else {
            String::from("Tarefa interrompida ou servidor parado")
        },
        detail: if last_relevant.is_empty() {
            if record.active {
                String::from("Comando enviado; aguardando o primeiro relatório de progresso.")
            } else {
                String::from("Não há processamento consumindo CPU agora.")
            }
        } else {
            last_relevant
        },
        eta_seconds,
        started_unix: Some(record.started_unix),
    }
}

#[tauri::command]
fn preview_chunk_generation(
    world: String,
    shape: String,
    x: i64,
    z: i64,
    radius: u32,
) -> Result<ChunkGenerationPreview, String> {
    let world = validated_server_identifier(&world, "O mundo")?;
    let shape = match shape.trim().to_ascii_lowercase().as_str() {
        "square" => String::from("square"),
        "circle" => String::from("circle"),
        _ => return Err(String::from("Escolha formato quadrado ou circular.")),
    };
    if !(16..=50_000).contains(&radius) {
        return Err(String::from(
            "O raio precisa ficar entre 16 e 50.000 blocos.",
        ));
    }
    let radius_chunks = (radius as f64 / 16.0).ceil() as u64;
    let chunks = if shape == "circle" {
        (std::f64::consts::PI * radius_chunks.pow(2) as f64).ceil() as u64
    } else {
        radius_chunks.saturating_mul(2).saturating_add(1).pow(2)
    };
    let estimated_disk_gb = chunks as f64 * 0.00035;
    let status = server_map_status_for(&read_server_profile())?;
    let enough_space = status.world_available_disk_gb >= estimated_disk_gb + 5.0;
    let safe = status.world_generation_safe && enough_space;
    let message = if !status.world_generation_safe {
        status.world_generation_message.clone()
    } else if !enough_space {
        format!(
            "Estimativa de {estimated_disk_gb:.1} GB mais 5 GB de margem excede o espaço livre."
        )
    } else {
        format!(
            "Aproximadamente {chunks} chunks; estimativa conservadora de {estimated_disk_gb:.1} GB."
        )
    };
    Ok(ChunkGenerationPreview {
        command: format!("chunky start {world} {shape} {x} {z} {radius}"),
        chunks,
        estimated_disk_gb,
        available_disk_gb: status.world_available_disk_gb,
        safe,
        message,
    })
}

#[tauri::command]
fn start_chunk_generation(
    world: String,
    shape: String,
    x: i64,
    z: i64,
    radius: u32,
) -> Result<String, String> {
    let profile = read_server_profile();
    let status = server_map_status_for(&profile)?;
    if !status.chunky_installed {
        return Err(String::from(
            "Chunky não foi encontrado. Instale o mod/plugin antes de gerar terreno.",
        ));
    }
    let preview = preview_chunk_generation(world, shape, x, z, radius)?;
    if !preview.safe {
        return Err(preview.message);
    }
    if !server_status().running {
        return Err(String::from("Inicie o servidor antes de gerar chunks."));
    }
    start_map_task(
        &profile,
        "world-generation",
        "chunky",
        "Gerando chunks do mundo",
    );
    if let Err(error) = send_server_command(preview.command.clone()) {
        update_map_task_active(&profile, false);
        return Err(error);
    }
    Ok(preview.command)
}

#[tauri::command]
fn control_chunk_generation(action: String, world: Option<String>) -> Result<String, String> {
    if !server_status().running {
        return Err(String::from("Inicie o servidor para controlar o Chunky."));
    }
    let action = match action.trim().to_ascii_lowercase().as_str() {
        "pause" | "continue" | "cancel" | "progress" => action.trim().to_ascii_lowercase(),
        _ => return Err(String::from("Ação do Chunky inválida.")),
    };
    let world = world
        .as_deref()
        .map(|value| validated_server_identifier(value, "O mundo"))
        .transpose()?;
    let command = if action == "progress" {
        String::from("chunky progress")
    } else if let Some(world) = world {
        format!("chunky {action} {world}")
    } else {
        format!("chunky {action}")
    };
    send_server_command(command.clone())?;
    if matches!(action.as_str(), "cancel") {
        update_map_task_active(&read_server_profile(), false);
    }
    Ok(command)
}

fn existing_bluemap_config_directory(directory: &Path) -> Option<PathBuf> {
    [
        directory.join("config/bluemap"),
        directory.join("plugins/BlueMap"),
        directory.join("bluemap"),
    ]
    .into_iter()
    .find(|path| path.is_dir())
}

#[tauri::command]
fn render_bluemap(
    world: String,
    x: i64,
    z: i64,
    radius: u32,
    threads: u32,
    force: bool,
) -> Result<String, String> {
    let world = validated_server_identifier(&world, "O mundo/mapa")?;
    if !(16..=100_000).contains(&radius) {
        return Err(String::from(
            "O raio de renderização precisa ficar entre 16 e 100.000 blocos.",
        ));
    }
    let profile = read_server_profile();
    let status = server_map_status_for(&profile)?;
    if !status.bluemap_enabled {
        return Err(String::from("Ative o BlueMap antes de renderizar."));
    }
    if !status.bluemap_storage_safe {
        return Err(status.bluemap_storage_message);
    }
    let max_threads = status.bluemap_max_threads.max(1);
    if threads == 0 || threads > max_threads {
        return Err(format!("Escolha entre 1 e {max_threads} threads."));
    }
    if !server_status().running {
        return Err(String::from(
            "Inicie o servidor antes de renderizar o mapa.",
        ));
    }
    let directory = current_server_directory(&profile)?;
    let config_directory = existing_bluemap_config_directory(&directory)
        .ok_or_else(|| String::from("A configuração do BlueMap ainda não foi criada."))?;
    set_hocon_setting(
        &config_directory.join("core.conf"),
        "render-thread-count",
        &threads.to_string(),
    )?;
    let map_config = config_directory.join("maps").join(format!("{world}.conf"));
    if map_config.is_file() {
        set_hocon_setting(&map_config, "start-pos", &format!("{{ x: {x}, z: {z} }}"))?;
    }
    let command = if force {
        format!("bluemap force-update {world} {x} {z} {radius}")
    } else {
        format!("bluemap update {world} {x} {z} {radius}")
    };
    start_map_task(
        &profile,
        "map-render",
        "bluemap",
        "Renderizando área 3D sob demanda",
    );
    let commands = [
        String::from("bluemap stop"),
        String::from("bluemap tasks cancel all"),
        String::from("bluemap reload light"),
        String::from("bluemap start"),
        command.clone(),
        String::from("bluemap"),
    ];
    if let Err(error) = commands
        .into_iter()
        .try_for_each(|command| send_server_command_for_profile(&profile, command))
    {
        update_map_task_active(&profile, false);
        return Err(error);
    }
    Ok(command)
}

#[tauri::command]
fn control_bluemap(
    action: String,
    world: Option<String>,
    x: Option<i64>,
    z: Option<i64>,
) -> Result<String, String> {
    if !server_status().running {
        return Err(String::from("Inicie o servidor para controlar o BlueMap."));
    }
    let command = match action.trim().to_ascii_lowercase().as_str() {
        "status" => String::from("bluemap"),
        "pause" => String::from("bluemap stop"),
        "resume" => String::from("bluemap start"),
        "tasks" => String::from("bluemap tasks"),
        "cancel" => String::from("bluemap tasks cancel all"),
        "reload" => String::from("bluemap reload light"),
        "freeze" | "unfreeze" => {
            let map = validated_server_identifier(world.as_deref().unwrap_or("world"), "O mapa")?;
            format!("bluemap {} {map}", action.trim().to_ascii_lowercase())
        }
        "troubleshoot" => {
            let map = validated_server_identifier(world.as_deref().unwrap_or("world"), "O mapa")?;
            format!(
                "bluemap troubleshoot {map} {} {}",
                x.unwrap_or_default(),
                z.unwrap_or_default()
            )
        }
        _ => return Err(String::from("Ação do BlueMap inválida.")),
    };
    send_server_command(command.clone())?;
    if matches!(
        action.trim().to_ascii_lowercase().as_str(),
        "pause" | "cancel"
    ) {
        update_map_task_active(&read_server_profile(), false);
    }
    Ok(command)
}

fn server_tool_platform(profile: &ServerProfile) -> Result<(&'static str, &'static str), String> {
    let directory = PathBuf::from(&profile.directory);
    let software = if matches!(profile.software.as_str(), "modded" | "unknown") {
        detected_server_software(&directory, resolved_server_target(profile).ok().as_deref())
    } else {
        profile.software.to_ascii_lowercase()
    };
    match software.as_str() {
        "paper" => Ok(("paper", "plugins")),
        "fabric" => Ok(("fabric", "mods")),
        "quilt" => Ok(("quilt", "mods")),
        "forge" => Ok(("forge", "mods")),
        "neoforge" => Ok(("neoforge", "mods")),
        "vanilla" => Err(String::from(
            "Vanilla não carrega plugins ou mods. Troque o servidor gerenciado para Paper ou Fabric primeiro.",
        )),
        _ => Err(String::from(
            "O loader deste servidor não foi identificado com segurança. Reimporte após conferir o script ou instale manualmente.",
        )),
    }
}

async fn resolve_server_tool_artifact(
    client: &reqwest::Client,
    project_id: &str,
    project: &str,
    license: &str,
    game_version: &str,
    loader: &str,
) -> Result<ServerToolArtifact, String> {
    let loaders = serde_json::to_string(&[loader]).map_err(|error| error.to_string())?;
    let game_versions =
        serde_json::to_string(&[game_version]).map_err(|error| error.to_string())?;
    let versions: Value = client
        .get(format!(
            "https://api.modrinth.com/v2/project/{project_id}/version"
        ))
        .query(&[
            ("loaders", loaders.as_str()),
            ("game_versions", game_versions.as_str()),
            ("include_changelog", "false"),
        ])
        .send()
        .await
        .map_err(|error| format!("Falha ao consultar {project} no Modrinth: {error}"))?
        .error_for_status()
        .map_err(|error| format!("Modrinth não encontrou {project}: {error}"))?
        .json()
        .await
        .map_err(|error| format!("Resposta inválida do Modrinth para {project}: {error}"))?;
    let mut compatible: Vec<&Value> = versions
        .as_array()
        .ok_or_else(|| String::from("O Modrinth devolveu uma lista de versões inválida."))?
        .iter()
        .filter(|version| {
            let listed = version
                .get("status")
                .and_then(Value::as_str)
                .is_none_or(|status| status == "listed");
            let exact_game = version
                .get("game_versions")
                .and_then(Value::as_array)
                .is_some_and(|items| items.iter().any(|item| item.as_str() == Some(game_version)));
            let exact_loader = version
                .get("loaders")
                .and_then(Value::as_array)
                .is_some_and(|items| items.iter().any(|item| item.as_str() == Some(loader)));
            listed && exact_game && exact_loader
        })
        .collect();
    compatible.sort_by(|left, right| {
        let left_release = left.get("version_type").and_then(Value::as_str) == Some("release");
        let right_release = right.get("version_type").and_then(Value::as_str) == Some("release");
        left_release.cmp(&right_release).then_with(|| {
            left.get("date_published")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .cmp(
                    right
                        .get("date_published")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                )
        })
    });
    let version = compatible.pop().ok_or_else(|| {
        format!(
            "{project} não possui uma versão exata para Minecraft {game_version} com {loader}. Nenhuma versão aproximada foi instalada."
        )
    })?;
    let file = version
        .get("files")
        .and_then(Value::as_array)
        .and_then(|files| {
            files
                .iter()
                .find(|file| file.get("primary").and_then(Value::as_bool) == Some(true))
                .or_else(|| files.first())
        })
        .ok_or_else(|| format!("{project} não publicou um arquivo para download."))?;
    let filename = file
        .get("filename")
        .and_then(Value::as_str)
        .filter(|filename| {
            filename.to_ascii_lowercase().ends_with(".jar")
                && Path::new(filename)
                    .file_name()
                    .and_then(|name| name.to_str())
                    == Some(*filename)
        })
        .ok_or_else(|| format!("{project} devolveu um nome de JAR inválido."))?;
    let download_url = file
        .get("url")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{project} não informou a URL de download."))?;
    validate_modrinth_download_url(download_url)?;
    let sha512 = file
        .get("hashes")
        .and_then(|hashes| hashes.get("sha512"))
        .and_then(Value::as_str)
        .filter(|hash| {
            hash.len() == 128 && hash.chars().all(|character| character.is_ascii_hexdigit())
        })
        .ok_or_else(|| format!("{project} não informou um SHA-512 válido."))?;
    let size = file
        .get("size")
        .and_then(Value::as_u64)
        .filter(|size| *size > 0 && *size <= 256 * 1024 * 1024)
        .ok_or_else(|| format!("O tamanho publicado de {project} é inválido ou muito grande."))?;
    Ok(ServerToolArtifact {
        project: project.to_owned(),
        version: version
            .get("version_number")
            .and_then(Value::as_str)
            .or_else(|| version.get("id").and_then(Value::as_str))
            .unwrap_or("desconhecida")
            .to_owned(),
        filename: filename.to_owned(),
        download_url: download_url.to_owned(),
        sha512: sha512.to_owned(),
        size,
        license: license.to_owned(),
    })
}

fn sha512_file(path: &Path) -> Option<String> {
    let mut file = fs::File::open(path).ok()?;
    let mut hasher = Sha512::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).ok()?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Some(format!("{:x}", hasher.finalize()))
}

async fn stage_server_tool_artifact(
    app: &tauri::AppHandle,
    client: &reqwest::Client,
    artifact: &ServerToolArtifact,
    destination: &Path,
    start: u8,
    end: u8,
) -> Result<PathBuf, String> {
    let part = destination.with_file_name(format!(".{}.part", artifact.filename));
    let _ = fs::remove_file(&part);
    let response = client
        .get(&artifact.download_url)
        .send()
        .await
        .map_err(|error| format!("Falha ao baixar {}: {error}", artifact.project))?
        .error_for_status()
        .map_err(|error| format!("Download de {} recusado: {error}", artifact.project))?;
    if response
        .content_length()
        .is_some_and(|size| size != artifact.size || size > 256 * 1024 * 1024)
    {
        return Err(format!(
            "O tamanho recebido de {} não corresponde ao publicado pelo Modrinth.",
            artifact.project
        ));
    }
    let result: Result<(), String> = async {
        let mut file = fs::File::create(&part).map_err(|error| error.to_string())?;
        let mut hasher = Sha512::new();
        let mut downloaded = 0_u64;
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|error| error.to_string())?;
            downloaded = downloaded.saturating_add(chunk.len() as u64);
            if downloaded > artifact.size || downloaded > 256 * 1024 * 1024 {
                return Err(format!(
                    "O download de {} ultrapassou o tamanho publicado.",
                    artifact.project
                ));
            }
            file.write_all(&chunk).map_err(|error| error.to_string())?;
            hasher.update(&chunk);
            let percent =
                start + ((downloaded as f64 / artifact.size as f64 * f64::from(end - start)) as u8);
            emit_progress(
                app,
                "install-server-map-stack",
                format!("Baixando {}", artifact.project),
                percent.min(end),
                false,
            );
        }
        file.flush().map_err(|error| error.to_string())?;
        drop(file);
        if downloaded != artifact.size {
            return Err(format!(
                "O download de {} terminou incompleto.",
                artifact.project
            ));
        }
        let actual = format!("{:x}", hasher.finalize());
        if !actual.eq_ignore_ascii_case(&artifact.sha512) {
            return Err(format!(
                "{} não passou na verificação SHA-512 do Modrinth.",
                artifact.project
            ));
        }
        let mut signature = [0_u8; 2];
        fs::File::open(&part)
            .and_then(|mut file| file.read_exact(&mut signature))
            .map_err(|error| error.to_string())?;
        if &signature != b"PK" {
            return Err(format!("{} não é um arquivo JAR válido.", artifact.project));
        }
        Ok(())
    }
    .await;
    if let Err(error) = result {
        let _ = fs::remove_file(&part);
        return Err(error);
    }
    Ok(part)
}

fn quoted_hocon_value(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

fn quoted_hocon_path(path: &Path) -> String {
    quoted_hocon_value(&path.to_string_lossy())
}

fn set_hocon_setting(path: &Path, key: &str, value: &str) -> Result<(), String> {
    let content = fs::read_to_string(path).unwrap_or_default();
    let mut replaced = false;
    let mut lines = Vec::new();
    for line in content.lines() {
        let clean = line.trim();
        let matches_key = !clean.starts_with('#')
            && !clean.starts_with("//")
            && clean
                .split_once([':', '='])
                .is_some_and(|(name, _)| name.trim() == key);
        if matches_key {
            if !replaced {
                lines.push(format!("{key}: {value}"));
                replaced = true;
            }
        } else {
            lines.push(line.to_owned());
        }
    }
    if !replaced {
        lines.push(format!("{key}: {value}"));
    }
    replace_text_file(path, &format!("{}\n", lines.join("\n")))
}

fn first_available_local_port(preferred: u16) -> Result<u16, String> {
    (preferred..preferred.saturating_add(100))
        .find(|port| {
            TcpStream::connect_timeout(
                &SocketAddr::from(([127, 0, 0, 1], *port)),
                Duration::from_millis(50),
            )
            .is_err()
        })
        .ok_or_else(|| String::from("Não há uma porta local livre entre 8100 e 8199."))
}

fn write_bluemap_map_config(
    path: &Path,
    world: &Path,
    dimension: &str,
    name: &str,
    sorting: i32,
) -> Result<(), String> {
    if path.is_file() {
        return Ok(());
    }
    replace_text_file(
        path,
        &format!(
            "world: {}\ndimension: {}\nname: {}\nsorting: {sorting}\nstorage: \"file\"\nenable-perspective-view: true\nenable-flat-view: true\nenable-free-flight-view: true\nenable-hires: true\n",
            quoted_hocon_path(world),
            quoted_hocon_value(dimension),
            quoted_hocon_value(name)
        ),
    )
}

fn configure_bluemap(profile: &ServerProfile, directory: &Path) -> Result<u16, String> {
    let heavy = server_maps_root(profile)?;
    let free = available_disk_gb(&heavy);
    if free < 5.0 {
        return Err(String::from(
            "O BlueMap precisa de pelo menos 5 GB livres no HD selecionado.",
        ));
    }
    let (loader, _) = server_tool_platform(profile)?;
    let config_directory = if loader == "paper" {
        directory.join("plugins").join("BlueMap")
    } else {
        directory.join("config").join("bluemap")
    };
    let maps_directory = config_directory.join("maps");
    let storages_directory = config_directory.join("storages");
    fs::create_dir_all(&maps_directory).map_err(|error| error.to_string())?;
    fs::create_dir_all(&storages_directory).map_err(|error| error.to_string())?;
    let data_root = heavy.join("bluemap");
    let webroot = data_root.join("web");
    let map_storage = webroot.join("maps");
    fs::create_dir_all(&map_storage).map_err(|error| error.to_string())?;
    let existing_port = configured_port(
        directory,
        &[
            "config/bluemap/webserver.conf",
            "plugins/BlueMap/webserver.conf",
            "bluemap/webserver.conf",
        ],
        &["port"],
    );
    let port = existing_port.unwrap_or(first_available_local_port(8100)?);
    let core = config_directory.join("core.conf");
    set_hocon_setting(&core, "accept-download", "true")?;
    set_hocon_setting(&core, "data", &quoted_hocon_path(&data_root.join("data")))?;
    set_hocon_setting(&core, "render-thread-count", "1")?;
    set_hocon_setting(&core, "scan-for-mod-resources", "true")?;
    let webserver = config_directory.join("webserver.conf");
    set_hocon_setting(&webserver, "enabled", "true")?;
    set_hocon_setting(&webserver, "webroot", &quoted_hocon_path(&webroot))?;
    set_hocon_setting(&webserver, "port", &port.to_string())?;
    let webapp = config_directory.join("webapp.conf");
    set_hocon_setting(&webapp, "enabled", "true")?;
    set_hocon_setting(&webapp, "webroot", &quoted_hocon_path(&webroot))?;
    set_hocon_setting(&webapp, "default-to-flat-view", "false")?;
    let storage = storages_directory.join("file.conf");
    set_hocon_setting(&storage, "storage-type", "file")?;
    set_hocon_setting(&storage, "root", &quoted_hocon_path(&map_storage))?;
    set_hocon_setting(&storage, "compression", "gzip")?;
    let plugin = config_directory.join("plugin.conf");
    set_hocon_setting(&plugin, "live-player-markers", "true")?;
    set_hocon_setting(&plugin, "hide-vanished", "true")?;
    set_hocon_setting(&plugin, "hide-invisible", "true")?;
    set_hocon_setting(&plugin, "player-render-limit", "1")?;
    let properties = read_server_properties(directory);
    let world_name = properties
        .get("level-name")
        .filter(|name| !name.trim().is_empty())
        .cloned()
        .unwrap_or_else(|| String::from("world"));
    let map_id = {
        let safe = safe_directory_name(&world_name);
        if safe.is_empty() {
            String::from("world")
        } else {
            safe
        }
    };
    let overworld = fs::canonicalize(directory.join(&world_name))
        .unwrap_or_else(|_| directory.join(&world_name));
    let nether = fs::canonicalize(directory.join(format!("{world_name}_nether")))
        .unwrap_or_else(|_| overworld.clone());
    let end = fs::canonicalize(directory.join(format!("{world_name}_the_end")))
        .unwrap_or_else(|_| overworld.clone());
    write_bluemap_map_config(
        &maps_directory.join(format!("{map_id}.conf")),
        &overworld,
        "minecraft:overworld",
        &world_name,
        0,
    )?;
    write_bluemap_map_config(
        &maps_directory.join(format!("{map_id}_nether.conf")),
        &nether,
        "minecraft:the_nether",
        &format!("{world_name} Nether"),
        1,
    )?;
    write_bluemap_map_config(
        &maps_directory.join(format!("{map_id}_the_end.conf")),
        &end,
        "minecraft:the_end",
        &format!("{world_name} The End"),
        2,
    )?;
    Ok(port)
}

fn bluemap_config_directory(profile: &ServerProfile, directory: &Path) -> Result<PathBuf, String> {
    let (loader, _) = server_tool_platform(profile)?;
    Ok(if loader == "paper" {
        directory.join("plugins").join("BlueMap")
    } else {
        directory.join("config").join("bluemap")
    })
}

#[tauri::command]
fn configure_bluemap_area(
    world: String,
    x: i64,
    z: i64,
    radius_chunks: u32,
) -> Result<BlueMapAreaConfig, String> {
    let _operation = begin_server_operation()?;
    ensure_server_stopped_for_profile_change()?;
    if x.unsigned_abs() > 30_000_000 || z.unsigned_abs() > 30_000_000 {
        return Err(String::from(
            "As coordenadas ultrapassam o limite do mundo do Minecraft.",
        ));
    }
    if !(1..=3_125).contains(&radius_chunks) {
        return Err(String::from("Escolha um raio entre 1 e 3.125 chunks."));
    }
    let map = validated_server_identifier(&world, "O mapa")?;
    if map.contains(':') {
        return Err(String::from(
            "O identificador do mapa não pode conter dois-pontos.",
        ));
    }
    let profile = read_server_profile();
    let status = server_map_status_for(&profile)?;
    if !status.bluemap_installed {
        return Err(String::from(
            "Instale o BlueMap antes de definir a área 3D.",
        ));
    }
    let directory = current_server_directory(&profile)?;
    let config_directory = bluemap_config_directory(&profile, &directory)?;
    let config_path = config_directory.join("maps").join(format!("{map}.conf"));
    if !config_path.is_file() {
        return Err(format!(
            "O mapa {map} ainda não possui configuração do BlueMap. Finalize a configuração 3D primeiro."
        ));
    }
    let radius_blocks = radius_chunks.saturating_mul(16);
    set_hocon_setting(&config_path, "start-pos", &format!("{{x: {x}, z: {z}}}"))?;
    let plugin = config_directory.join("plugin.conf");
    set_hocon_setting(&plugin, "player-render-limit", "1")?;
    Ok(BlueMapAreaConfig {
        map,
        x,
        z,
        radius_blocks,
        radius_chunks,
        config_path: config_path.to_string_lossy().into_owned(),
    })
}

#[tauri::command]
async fn install_server_map_stack(
    app: tauri::AppHandle,
    provider: String,
    accept_mojang_download: Option<bool>,
) -> Result<ServerToolInstallResult, String> {
    let _operation = begin_server_operation()?;
    ensure_server_stopped_for_profile_change()?;
    let provider = provider.trim().to_ascii_lowercase();
    if !matches!(provider.as_str(), "chunky" | "bluemap") {
        return Err(String::from(
            "Escolha Chunky para gerar terreno ou BlueMap para a visualização 3D.",
        ));
    }
    if provider == "bluemap" && accept_mojang_download != Some(true) {
        return Err(String::from(
            "Confirme a licença do Minecraft e o download de recursos da Mojang antes de configurar o BlueMap.",
        ));
    }
    let profile = read_server_profile();
    let directory = current_server_directory(&profile)?;
    let (loader, folder) = server_tool_platform(&profile)?;
    let items = server_content_items(&profile)?;
    let enabled_names: Vec<String> = items
        .iter()
        .filter(|item| item.enabled)
        .map(|item| item.name.to_ascii_lowercase())
        .collect();
    let has = |needle: &str| enabled_names.iter().any(|name| name.contains(needle));
    let status = server_map_status_for(&profile)?;
    if provider == "bluemap" && !status.bluemap_storage_safe {
        return Err(status.bluemap_storage_message);
    }

    let mut already_present = Vec::new();
    let mut requested: Vec<(&str, &str, &str)> = Vec::new();
    if loader == "fabric" && !(has("fabric-api") || has("fabric_api")) {
        requested.push(("P7dR8mSH", "Fabric API", "Apache-2.0"));
    } else if loader == "fabric" {
        already_present.push(String::from("Fabric API"));
    }
    if provider == "chunky" && !has("chunky") {
        requested.push(("fALzjamp", "Chunky", "GPL-3.0-only"));
    } else if provider == "chunky" {
        already_present.push(String::from("Chunky"));
    }
    if provider == "bluemap" && !has("bluemap") {
        requested.push(("swbUV1cr", "BlueMap", "MIT"));
    } else {
        if provider == "bluemap" {
            already_present.push(String::from("BlueMap"));
        }
    }

    if requested.is_empty() {
        if provider == "bluemap" {
            configure_bluemap(&profile, &directory)?;
        }
        return Ok(ServerToolInstallResult {
            installed: Vec::new(),
            already_present,
            source: String::from("Modrinth CDN oficial"),
        });
    }
    emit_progress(
        &app,
        "install-server-map-stack",
        "Resolvendo versões exatas no Modrinth",
        4,
        false,
    );
    let client = reqwest::Client::builder()
        .user_agent(VEX_USER_AGENT)
        .build()
        .map_err(|error| error.to_string())?;
    let target_directory = directory.join(folder);
    let mut planned = Vec::<(ServerToolArtifact, PathBuf)>::new();
    for (project_id, project, license) in requested {
        let artifact = resolve_server_tool_artifact(
            &client,
            project_id,
            project,
            license,
            &profile.version,
            loader,
        )
        .await?;
        let destination = target_directory.join(&artifact.filename);
        if destination.exists() {
            if sha512_file(&destination)
                .is_some_and(|hash| hash.eq_ignore_ascii_case(&artifact.sha512))
            {
                already_present.push(artifact.project);
                continue;
            }
            return Err(format!(
                "Já existe {} com conteúdo diferente. O VEX não substituiu o arquivo.",
                destination.display()
            ));
        }
        planned.push((artifact, destination));
    }

    fs::create_dir_all(&target_directory).map_err(|error| error.to_string())?;
    let planned_count = planned.len().max(1);
    let mut staged = Vec::<(ServerToolArtifact, PathBuf, PathBuf)>::new();
    for (index, (artifact, destination)) in planned.into_iter().enumerate() {
        let start = 12 + ((index as u16 * 76) / planned_count as u16) as u8;
        let end = 12 + (((index + 1) as u16 * 76) / planned_count as u16) as u8;
        match stage_server_tool_artifact(&app, &client, &artifact, &destination, start, end).await {
            Ok(part) => staged.push((artifact, destination, part)),
            Err(error) => {
                for (_, _, part) in &staged {
                    let _ = fs::remove_file(part);
                }
                return Err(error);
            }
        }
    }

    let mut installed = Vec::new();
    let mut committed = Vec::<PathBuf>::new();
    for (artifact, destination, part) in &staged {
        if let Err(error) = fs::rename(part, destination) {
            for path in &committed {
                let _ = fs::remove_file(path);
            }
            for (_, _, pending) in &staged {
                let _ = fs::remove_file(pending);
            }
            return Err(format!(
                "Não foi possível concluir a instalação; os novos arquivos foram revertidos: {error}"
            ));
        }
        committed.push(destination.clone());
        installed.push(ServerToolInstallItem {
            project: artifact.project.clone(),
            version: artifact.version.clone(),
            filename: artifact.filename.clone(),
            destination: destination.to_string_lossy().into_owned(),
            license: artifact.license.clone(),
        });
    }
    if provider == "bluemap" {
        if let Err(error) = configure_bluemap(&profile, &directory) {
            for path in &committed {
                let _ = fs::remove_file(path);
            }
            return Err(format!(
                "O BlueMap não pôde ser configurado e os novos JARs foram removidos: {error}"
            ));
        }
    }
    emit_progress(
        &app,
        "install-server-map-stack",
        if provider == "bluemap" {
            "BlueMap 3D configurado"
        } else {
            "Chunky preparado"
        },
        100,
        true,
    );
    Ok(ServerToolInstallResult {
        installed,
        already_present,
        source: String::from("Modrinth CDN oficial"),
    })
}

fn normalized_server_registry(mut registry: ServerRegistry) -> ServerRegistry {
    if registry.servers.is_empty() {
        registry.active_id.clear();
        return registry;
    }
    let mut used = HashSet::new();
    for (index, profile) in registry.servers.iter_mut().enumerate() {
        if profile.id.trim().is_empty() || !used.insert(profile.id.clone()) {
            let digest = Sha256::digest(
                format!("{}:{}:{index}", profile.directory, profile.name).as_bytes(),
            );
            profile.id = format!("server-{}", &format!("{digest:x}")[..12]);
            used.insert(profile.id.clone());
        }
    }
    if !registry
        .servers
        .iter()
        .any(|profile| profile.id == registry.active_id)
    {
        registry.active_id = registry.servers[0].id.clone();
    }
    registry
}

fn read_server_registry_unlocked() -> ServerRegistry {
    if let Some(registry) = read_json_with_recovery::<ServerRegistry>(&server_registry_path()) {
        return normalized_server_registry(registry);
    }
    if let Some(profile) = read_json_with_recovery::<ServerProfile>(&server_profile_path()) {
        return normalized_server_registry(ServerRegistry {
            active_id: profile.id.clone(),
            servers: vec![profile],
        });
    }
    ServerRegistry::default()
}

fn read_server_registry() -> ServerRegistry {
    let _guard = server_registry_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    read_server_registry_unlocked()
}

fn sibling_work_file(path: &Path, suffix: &str) -> Option<PathBuf> {
    let parent = path.parent()?;
    let filename = path.file_name()?.to_str()?;
    Some(parent.join(format!(".{filename}.{suffix}")))
}

fn read_json_with_recovery<T: DeserializeOwned>(path: &Path) -> Option<T> {
    let backup = sibling_work_file(path, "bak");
    let temporary = sibling_work_file(path, "tmp");
    std::iter::once(path.to_path_buf())
        .chain(backup)
        .chain(temporary)
        .find_map(|candidate| {
            fs::read_to_string(candidate)
                .ok()
                .and_then(|raw| serde_json::from_str::<T>(&raw).ok())
        })
}

fn replace_text_file(path: &Path, content: &str) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| String::from("Caminho de arquivo inválido."))?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let temporary =
        sibling_work_file(path, "tmp").ok_or_else(|| String::from("Nome de arquivo inválido."))?;
    fs::write(&temporary, content).map_err(|error| error.to_string())?;

    #[cfg(not(windows))]
    {
        fs::rename(&temporary, path).map_err(|error| error.to_string())?;
    }
    #[cfg(windows)]
    {
        let backup = sibling_work_file(path, "bak")
            .ok_or_else(|| String::from("Nome de arquivo inválido."))?;
        let had_previous = path.exists();
        if had_previous {
            fs::copy(path, &backup).map_err(|error| error.to_string())?;
            fs::remove_file(path).map_err(|error| error.to_string())?;
        }
        if let Err(error) = fs::rename(&temporary, path) {
            if had_previous {
                let _ = fs::rename(&backup, path);
            }
            return Err(error.to_string());
        }
        if had_previous {
            let _ = fs::remove_file(backup);
        }
    }
    Ok(())
}

fn persist_server_registry_unlocked(registry: &ServerRegistry) -> Result<(), String> {
    let path = server_registry_path();
    fs::create_dir_all(
        path.parent()
            .ok_or_else(|| String::from("Caminho de servidor inválido."))?,
    )
    .map_err(|error| error.to_string())?;
    replace_text_file(
        &path,
        &serde_json::to_string_pretty(registry).map_err(|error| error.to_string())?,
    )?;
    if let Some(active) = registry
        .servers
        .iter()
        .find(|profile| profile.id == registry.active_id)
    {
        let _ = replace_text_file(
            &server_profile_path(),
            &serde_json::to_string_pretty(active).map_err(|error| error.to_string())?,
        );
    } else {
        let legacy = server_profile_path();
        let mut stale_paths = vec![legacy.clone()];
        if let Some(backup) = sibling_work_file(&legacy, "bak") {
            stale_paths.push(backup);
        }
        if let Some(temporary) = sibling_work_file(&legacy, "tmp") {
            stale_paths.push(temporary);
        }
        for stale in stale_paths {
            if stale.is_file() {
                let _ = fs::remove_file(stale);
            }
        }
    }
    Ok(())
}

fn persist_server_registry(registry: &ServerRegistry) -> Result<(), String> {
    let _guard = server_registry_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    persist_server_registry_unlocked(registry)
}

fn read_server_profile() -> ServerProfile {
    let registry = read_server_registry();
    registry
        .servers
        .iter()
        .find(|profile| profile.id == registry.active_id)
        .cloned()
        .unwrap_or_else(|| registry.servers.first().cloned().unwrap_or_default())
}

fn write_server_profile(profile: &ServerProfile) -> Result<(), String> {
    let _guard = server_registry_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut registry = read_server_registry_unlocked();
    let mut profile = profile.clone();
    if profile.id.trim().is_empty() {
        let digest = Sha256::digest(profile.directory.as_bytes());
        profile.id = format!("server-{}", &format!("{digest:x}")[..12]);
    }
    if let Some(existing) = registry
        .servers
        .iter_mut()
        .find(|existing| existing.id == profile.id)
    {
        *existing = profile.clone();
    } else {
        registry.servers.push(profile.clone());
    }
    registry.active_id = profile.id;
    persist_server_registry_unlocked(&normalized_server_registry(registry))
}

fn storage_folder_names() -> Vec<String> {
    [
        ".minecraft",
        "cache",
        "downloads",
        "exports",
        "instances",
        "logs",
        "maps",
        "runtimes",
        "servers",
    ]
    .into_iter()
    .map(String::from)
    .collect()
}

fn storage_status_for(root: &Path, strategy: &str, configured: bool) -> StorageStatus {
    StorageStatus {
        root: root.to_string_lossy().into_owned(),
        strategy: strategy.to_owned(),
        configured,
        available_gb: available_disk_gb(root),
        folders: storage_folder_names(),
    }
}

#[cfg(target_os = "linux")]
fn collect_linux_storage_disks(value: &Value, disks: &mut Vec<StorageDisk>) {
    let Some(items) = value.as_array() else {
        return;
    };
    for item in items {
        if item.get("ro").and_then(Value::as_bool) != Some(true) {
            let name = item.get("name").and_then(Value::as_str).unwrap_or("disco");
            let label = item
                .get("label")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .unwrap_or(name);
            let total_gb = item
                .get("size")
                .and_then(Value::as_u64)
                .map(|bytes| bytes as f64 / 1024.0 / 1024.0 / 1024.0)
                .unwrap_or_default();
            let removable = item.get("rm").and_then(Value::as_bool).unwrap_or(false);
            let mountpoints: Vec<&str> = item
                .get("mountpoints")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            let preferred_mount = mountpoints
                .iter()
                .copied()
                .find(|mount| {
                    mount.starts_with("/mnt/")
                        || mount.starts_with("/media/")
                        || mount.starts_with("/run/media/")
                })
                .or_else(|| {
                    mountpoints
                        .iter()
                        .copied()
                        .find(|mount| matches!(*mount, "/home" | "/"))
                });
            if let Some(mount) = preferred_mount {
                let mount_path = PathBuf::from(mount);
                let root = if matches!(mount, "/home" | "/") {
                    env::var("HOME")
                        .map(PathBuf::from)
                        .unwrap_or_else(|_| mount_path.clone())
                        .join(".local")
                        .join("share")
                        .join("vex-launcher")
                } else {
                    mount_path.join("VEX-Launcher").join("data")
                };
                let available_gb = available_disk_gb(&mount_path);
                if available_gb >= 2.0
                    && !disks
                        .iter()
                        .any(|disk| disk.root_path == root.to_string_lossy())
                {
                    disks.push(StorageDisk {
                        id: name.to_owned(),
                        label: format!("{} · {}", label, mount),
                        mount_path: mount.to_owned(),
                        root_path: root.to_string_lossy().into_owned(),
                        available_gb,
                        total_gb,
                        removable,
                        current: false,
                        recommended: false,
                    });
                }
            }
        }
        if let Some(children) = item.get("children") {
            collect_linux_storage_disks(children, disks);
        }
    }
}

#[cfg(target_os = "linux")]
fn storage_disks_internal() -> Vec<StorageDisk> {
    let mut disks = hidden_command("lsblk")
        .args([
            "-J",
            "-b",
            "-o",
            "NAME,LABEL,TYPE,SIZE,FSTYPE,MOUNTPOINTS,RM,RO",
        ])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| serde_json::from_slice::<Value>(&output.stdout).ok())
        .map(|value| {
            let mut disks = Vec::new();
            collect_linux_storage_disks(
                value.get("blockdevices").unwrap_or(&Value::Null),
                &mut disks,
            );
            disks
        })
        .unwrap_or_default();
    let current_root = storage_root().to_string_lossy().into_owned();
    for disk in &mut disks {
        disk.current = disk.root_path == current_root;
    }
    if let Some((index, _)) = disks
        .iter()
        .enumerate()
        .max_by(|(_, left), (_, right)| left.available_gb.total_cmp(&right.available_gb))
    {
        disks[index].recommended = true;
    }
    disks.sort_by(|left, right| {
        right
            .recommended
            .cmp(&left.recommended)
            .then_with(|| right.available_gb.total_cmp(&left.available_gb))
    });
    disks
}

#[cfg(windows)]
fn storage_disks_internal() -> Vec<StorageDisk> {
    let script = "Get-PSDrive -PSProvider FileSystem | Where-Object {$_.Root} | ForEach-Object {[PSCustomObject]@{Name=$_.Name;Root=$_.Root;Free=$_.Free;Used=$_.Used}} | ConvertTo-Json -Compress";
    let values = hidden_command("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| serde_json::from_slice::<Value>(&output.stdout).ok())
        .map(|value| match value {
            Value::Array(values) => values,
            Value::Object(_) => vec![value],
            _ => Vec::new(),
        })
        .unwrap_or_default();
    let current_root = storage_root().to_string_lossy().to_ascii_lowercase();
    let mut disks: Vec<StorageDisk> = values
        .into_iter()
        .filter_map(|value| {
            let name = value.get("Name")?.as_str()?;
            let mount = value.get("Root")?.as_str()?;
            let free = value
                .get("Free")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            let used = value
                .get("Used")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            let root = if name.eq_ignore_ascii_case("C") {
                env::var("LOCALAPPDATA")
                    .map(PathBuf::from)
                    .unwrap_or_else(|_| PathBuf::from(mount))
                    .join("VEX Launcher")
            } else {
                PathBuf::from(mount).join("VEX-Launcher").join("data")
            };
            let root_text = root.to_string_lossy().into_owned();
            Some(StorageDisk {
                id: name.to_owned(),
                label: format!("Disco {name}:"),
                mount_path: mount.to_owned(),
                root_path: root_text.clone(),
                available_gb: free as f64 / 1024.0 / 1024.0 / 1024.0,
                total_gb: free.saturating_add(used) as f64 / 1024.0 / 1024.0 / 1024.0,
                removable: false,
                current: current_root == root_text.to_ascii_lowercase(),
                recommended: false,
            })
        })
        .collect();
    if let Some((index, _)) = disks
        .iter()
        .enumerate()
        .max_by(|(_, left), (_, right)| left.available_gb.total_cmp(&right.available_gb))
    {
        disks[index].recommended = true;
    }
    disks.sort_by(|left, right| right.recommended.cmp(&left.recommended));
    disks
}

#[cfg(all(not(target_os = "linux"), not(windows)))]
fn storage_disks_internal() -> Vec<StorageDisk> {
    let root = storage_root();
    vec![StorageDisk {
        id: String::from("default"),
        label: String::from("Armazenamento padrão"),
        mount_path: root.to_string_lossy().into_owned(),
        root_path: root.to_string_lossy().into_owned(),
        available_gb: available_disk_gb(&root),
        total_gb: 0.0,
        removable: false,
        current: true,
        recommended: true,
    }]
}

#[tauri::command]
fn storage_status() -> StorageStatus {
    storage_status_for(
        &storage_root(),
        if storage_root_is_configured() {
            "selected-disk"
        } else {
            "automatic-default"
        },
        storage_root_is_configured(),
    )
}

#[tauri::command]
fn list_storage_disks() -> Vec<StorageDisk> {
    storage_disks_internal()
}

#[tauri::command]
fn select_storage_disk(root_path: String) -> Result<StorageStatus, String> {
    let selected = storage_disks_internal()
        .into_iter()
        .find(|disk| disk.root_path == root_path)
        .ok_or_else(|| String::from("Escolha um dos discos detectados pelo VEX."))?;
    let root = PathBuf::from(&selected.root_path);
    ensure_storage_layout(&root)?;
    fs::create_dir_all(root.join(".minecraft")).map_err(|error| error.to_string())?;
    persist_storage_root_pointer(&root)?;
    replace_text_file(
        &root.join("storage-layout.json"),
        &serde_json::to_string_pretty(&serde_json::json!({
            "version": 1,
            "root": root,
            "folders": storage_folder_names(),
        }))
        .map_err(|error| error.to_string())?,
    )?;
    Ok(storage_status_for(&root, "selected-disk", true))
}

#[tauri::command]
fn get_launcher_settings() -> LauncherSettings {
    read_settings()
}

#[tauri::command]
fn set_game_directory(game_directory: String) -> Result<LauncherSettings, String> {
    let path = PathBuf::from(game_directory.trim());
    if !path.exists() {
        fs::create_dir_all(&path).map_err(|error| error.to_string())?;
    }
    let mut settings = read_settings();
    settings.game_directory = path.to_string_lossy().into_owned();
    write_settings(&settings)?;
    Ok(settings)
}

#[tauri::command]
fn set_runtime_preferences(
    mangohud_enabled: bool,
    minimize_on_launch: bool,
) -> Result<LauncherSettings, String> {
    let mut settings = read_settings();
    settings.mangohud_enabled = mangohud_enabled;
    settings.minimize_on_launch = minimize_on_launch;
    write_settings(&settings)?;
    Ok(settings)
}

#[tauri::command]
fn open_path(path: String) -> Result<(), String> {
    let target = PathBuf::from(path.trim());
    if !target.exists() {
        return Err(String::from("O caminho não existe."));
    }
    #[cfg(windows)]
    {
        let mut command = hidden_command("explorer.exe");
        if target.is_file() {
            command.arg("/select,");
        }
        command
            .arg(target)
            .spawn()
            .map_err(|error| error.to_string())?;
    }
    #[cfg(not(windows))]
    hidden_command("xdg-open")
        .arg(if target.is_file() {
            target.parent().unwrap_or(&target)
        } else {
            &target
        })
        .spawn()
        .map_err(|error| error.to_string())?;
    Ok(())
}

#[tauri::command]
fn open_url(url: String) -> Result<(), String> {
    let parsed = reqwest::Url::parse(url.trim()).map_err(|_| String::from("Link inválido."))?;
    let local_http = parsed.scheme() == "http"
        && matches!(parsed.host_str(), Some("127.0.0.1") | Some("localhost"));
    if parsed.scheme() != "https" && !local_http {
        return Err(String::from(
            "Somente links HTTPS ou mapas locais em 127.0.0.1/localhost são permitidos.",
        ));
    }
    let url = parsed.to_string();
    #[cfg(windows)]
    hidden_command("explorer.exe")
        .arg(&url)
        .spawn()
        .map_err(|error| error.to_string())?;
    #[cfg(not(windows))]
    hidden_command("xdg-open")
        .arg(&url)
        .spawn()
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn microsoft_login_url() -> String {
    format!(
        "https://login.live.com/oauth20_authorize.srf?client_id={MICROSOFT_CLIENT_ID}&response_type=code&scope=XboxLive.signin%20offline_access&redirect_uri=https%3A%2F%2Flogin.live.com%2Foauth20_desktop.srf"
    )
}

#[cfg(windows)]
fn ensure_embedded_auth_helper() -> Result<PathBuf, String> {
    let directory = storage_root().join("auth-helper");
    fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    let files: [(&str, &[u8]); 5] = [
        (
            "VexMicrosoftAuth.exe",
            include_bytes!("../resources/auth-helper/VexMicrosoftAuth.exe"),
        ),
        (
            "VexMicrosoftAuth.exe.config",
            include_bytes!("../resources/auth-helper/VexMicrosoftAuth.exe.config"),
        ),
        (
            "Microsoft.Web.WebView2.Core.dll",
            include_bytes!("../resources/auth-helper/Microsoft.Web.WebView2.Core.dll"),
        ),
        (
            "Microsoft.Web.WebView2.WinForms.dll",
            include_bytes!("../resources/auth-helper/Microsoft.Web.WebView2.WinForms.dll"),
        ),
        (
            "WebView2Loader.dll",
            include_bytes!("../resources/auth-helper/WebView2Loader.dll"),
        ),
    ];
    for (name, bytes) in files {
        let path = directory.join(name);
        if fs::metadata(&path)
            .map(|metadata| metadata.len())
            .unwrap_or_default()
            != bytes.len() as u64
        {
            fs::write(path, bytes).map_err(|error| error.to_string())?;
        }
    }
    Ok(directory.join("VexMicrosoftAuth.exe"))
}

fn microsoft_auth_error(value: &Value, fallback: &str) -> String {
    value
        .get("error_description")
        .or_else(|| value.get("Message"))
        .or_else(|| value.get("errorMessage"))
        .or_else(|| value.get("error"))
        .and_then(Value::as_str)
        .unwrap_or(fallback)
        .to_owned()
}

async fn microsoft_token_from_code(
    client: &reqwest::Client,
    code: &str,
) -> Result<(String, String), String> {
    let response = client
        .post("https://login.live.com/oauth20_token.srf")
        .form(&[
            ("client_id", MICROSOFT_CLIENT_ID),
            ("code", code),
            ("grant_type", "authorization_code"),
            ("redirect_uri", MICROSOFT_REDIRECT_URI),
        ])
        .send()
        .await
        .map_err(|error| error.to_string())?;
    let status = response.status();
    let value: Value = response.json().await.map_err(|error| error.to_string())?;
    if !status.is_success() || value.get("error").is_some() {
        return Err(microsoft_auth_error(
            &value,
            "A Microsoft recusou o código de autorização.",
        ));
    }
    Ok((
        value
            .get("access_token")
            .and_then(Value::as_str)
            .ok_or_else(|| String::from("A Microsoft não retornou um token de acesso."))?
            .to_owned(),
        value
            .get("refresh_token")
            .and_then(Value::as_str)
            .ok_or_else(|| String::from("A Microsoft não retornou uma sessão renovável."))?
            .to_owned(),
    ))
}

async fn microsoft_token_from_refresh(
    client: &reqwest::Client,
    refresh_token: &str,
) -> Result<(String, String), String> {
    let response = client
        .post("https://login.live.com/oauth20_token.srf")
        .form(&[
            ("client_id", MICROSOFT_CLIENT_ID),
            ("refresh_token", refresh_token),
            ("grant_type", "refresh_token"),
            ("scope", "XboxLive.signin offline_access"),
        ])
        .send()
        .await
        .map_err(|error| error.to_string())?;
    let status = response.status();
    let value: Value = response.json().await.map_err(|error| error.to_string())?;
    if !status.is_success() || value.get("error").is_some() {
        return Err(microsoft_auth_error(
            &value,
            "Sua sessão Microsoft expirou. Entre novamente.",
        ));
    }
    Ok((
        value
            .get("access_token")
            .and_then(Value::as_str)
            .ok_or_else(|| String::from("A Microsoft não retornou um token de acesso."))?
            .to_owned(),
        value
            .get("refresh_token")
            .and_then(Value::as_str)
            .unwrap_or(refresh_token)
            .to_owned(),
    ))
}

async fn minecraft_account_from_microsoft_token(
    client: &reqwest::Client,
    microsoft_access_token: &str,
    refresh_token: String,
) -> Result<AuthenticatedMicrosoftAccount, String> {
    let response = client
        .post("https://user.auth.xboxlive.com/user/authenticate")
        .header("Accept", "application/json")
        .json(&serde_json::json!({
            "Properties": {
                "AuthMethod": "RPS",
                "SiteName": "user.auth.xboxlive.com",
                "RpsTicket": format!("d={microsoft_access_token}")
            },
            "RelyingParty": "http://auth.xboxlive.com",
            "TokenType": "JWT"
        }))
        .send()
        .await
        .map_err(|error| error.to_string())?;
    let status = response.status();
    let xbl: Value = response.json().await.map_err(|error| error.to_string())?;
    if !status.is_success() || xbl.get("XErr").is_some() {
        return Err(microsoft_auth_error(
            &xbl,
            "Não foi possível autenticar no Xbox Live.",
        ));
    }
    let xbl_token = xbl
        .get("Token")
        .and_then(Value::as_str)
        .ok_or_else(|| String::from("O Xbox Live não retornou um token."))?;
    let user_hash = xbl
        .pointer("/DisplayClaims/xui/0/uhs")
        .and_then(Value::as_str)
        .ok_or_else(|| String::from("O Xbox Live não retornou a identificação da conta."))?;

    let response = client
        .post("https://xsts.auth.xboxlive.com/xsts/authorize")
        .header("Accept", "application/json")
        .json(&serde_json::json!({
            "Properties": {
                "SandboxId": "RETAIL",
                "UserTokens": [xbl_token]
            },
            "RelyingParty": "rp://api.minecraftservices.com/",
            "TokenType": "JWT"
        }))
        .send()
        .await
        .map_err(|error| error.to_string())?;
    let status = response.status();
    let xsts: Value = response.json().await.map_err(|error| error.to_string())?;
    if !status.is_success() || xsts.get("XErr").is_some() {
        let message = match xsts.get("XErr").and_then(Value::as_u64) {
            Some(2_148_916_233) => {
                "Esta conta Microsoft ainda não possui um perfil do Xbox Live.".to_owned()
            }
            Some(2_148_916_238) => {
                "Esta conta infantil precisa de permissão nas configurações de família.".to_owned()
            }
            _ => microsoft_auth_error(&xsts, "Não foi possível autorizar o Xbox Live."),
        };
        return Err(message);
    }
    let xsts_token = xsts
        .get("Token")
        .and_then(Value::as_str)
        .ok_or_else(|| String::from("O Xbox Live não retornou a autorização XSTS."))?;

    let response = client
        .post("https://api.minecraftservices.com/authentication/login_with_xbox")
        .json(&serde_json::json!({
            "identityToken": format!("XBL3.0 x={user_hash};{xsts_token}")
        }))
        .send()
        .await
        .map_err(|error| error.to_string())?;
    let status = response.status();
    let minecraft_auth: Value = response.json().await.map_err(|error| error.to_string())?;
    if !status.is_success() || minecraft_auth.get("error").is_some() {
        return Err(microsoft_auth_error(
            &minecraft_auth,
            "Não foi possível entrar nos serviços do Minecraft.",
        ));
    }
    let minecraft_access_token = minecraft_auth
        .get("access_token")
        .and_then(Value::as_str)
        .ok_or_else(|| String::from("O Minecraft não retornou um token de acesso."))?
        .to_owned();

    let response = client
        .get("https://api.minecraftservices.com/minecraft/profile")
        .bearer_auth(&minecraft_access_token)
        .send()
        .await
        .map_err(|error| error.to_string())?;
    let status = response.status();
    let profile: Value = response.json().await.map_err(|error| error.to_string())?;
    if !status.is_success() || profile.get("error").is_some() {
        return Err(microsoft_auth_error(
            &profile,
            "Perfil Minecraft não encontrado. Verifique se esta conta possui o jogo.",
        ));
    }
    Ok(AuthenticatedMicrosoftAccount {
        username: profile
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| String::from("Perfil Minecraft sem nome."))?
            .to_owned(),
        uuid: profile
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| String::from("Perfil Minecraft sem UUID."))?
            .to_owned(),
        skin_url: profile
            .pointer("/skins/0/url")
            .and_then(Value::as_str)
            .map(str::to_owned),
        access_token: minecraft_access_token,
        refresh_token,
    })
}

async fn refresh_microsoft_account(
    client: &reqwest::Client,
) -> Result<AuthenticatedMicrosoftAccount, String> {
    let stored = read_microsoft_account()
        .ok_or_else(|| String::from("Entre com a Microsoft antes de iniciar esta instância."))?;
    let refresh_token = unprotect_secret(&stored.protected_refresh_token)?;
    let (microsoft_access_token, new_refresh_token) =
        microsoft_token_from_refresh(client, &refresh_token).await?;
    let account =
        minecraft_account_from_microsoft_token(client, &microsoft_access_token, new_refresh_token)
            .await?;
    write_microsoft_account(&account)?;
    let _ = cache_microsoft_skin(client, account.skin_url.as_deref()).await;
    Ok(account)
}

#[tauri::command]
fn get_microsoft_account() -> MicrosoftAccountStatus {
    microsoft_account_status_value()
}

#[tauri::command]
async fn get_microsoft_skin_data_url() -> Result<Option<String>, String> {
    if let Some(data_url) = image_data_url(&microsoft_skin_path(), "image/png") {
        return Ok(Some(data_url));
    }
    let Some(url) = read_microsoft_account()
        .and_then(|account| account.skin_url)
        .as_deref()
        .and_then(normalized_minecraft_texture_url)
    else {
        return Ok(None);
    };
    let bytes = reqwest::Client::builder()
        .user_agent(VEX_USER_AGENT)
        .build()
        .map_err(|error| error.to_string())?
        .get(&url)
        .send()
        .await
        .map_err(|error| error.to_string())?
        .error_for_status()
        .map_err(|error| error.to_string())?
        .bytes()
        .await
        .map_err(|error| error.to_string())?;
    if bytes.len() > 10 * 1024 * 1024 || !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err(String::from(
            "A skin Microsoft recebida não é um PNG válido.",
        ));
    }
    let path = microsoft_skin_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    fs::write(&path, &bytes).map_err(|error| error.to_string())?;
    Ok(image_data_url(&path, "image/png"))
}

#[cfg(windows)]
#[tauri::command]
fn begin_microsoft_login(app: tauri::AppHandle) -> Result<(), String> {
    let helper_name = "VexMicrosoftAuth.exe";
    let resource_helper = app
        .path()
        .resource_dir()
        .map_err(|error| error.to_string())?
        .join("auth-helper")
        .join(helper_name);
    let development_helper = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("resources")
        .join("auth-helper")
        .join(helper_name);
    let helper_path = if resource_helper.is_file() {
        resource_helper
    } else if development_helper.is_file() {
        development_helper
    } else {
        #[cfg(windows)]
        {
            ensure_embedded_auth_helper()?
        }
        #[cfg(not(windows))]
        {
            return Err(String::from(
                "O login Microsoft integrado ainda está disponível apenas no Windows.",
            ));
        }
    };

    let auth_dir = storage_root().join("auth");
    fs::create_dir_all(&auth_dir).map_err(|error| error.to_string())?;
    let result_path = auth_dir.join("microsoft-login-result.txt");
    if result_path.is_file() {
        fs::remove_file(&result_path).map_err(|error| error.to_string())?;
    }

    let mut helper_command = hidden_command(helper_path.as_os_str());
    helper_command
        .env_remove("WEBVIEW2_USER_DATA_FOLDER")
        .arg(microsoft_login_url())
        .arg(&result_path);
    let mut child = helper_command
        .spawn()
        .map_err(|error| format!("Não foi possível abrir o login Microsoft: {error}"))?;
    let app_for_result = app.clone();
    std::thread::spawn(move || {
        for _ in 0..1_200 {
            if result_path.is_file() {
                break;
            }
            if matches!(child.try_wait(), Ok(Some(_))) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }

        let result = fs::read_to_string(&result_path);
        let _ = fs::remove_file(&result_path);
        match result {
            Ok(value) => match reqwest::Url::parse(value.trim()) {
                Ok(url) => {
                    if let Some(code) = url
                        .query_pairs()
                        .find_map(|(key, value)| (key == "code").then(|| value.into_owned()))
                    {
                        let _ = app_for_result.emit("microsoft-auth-code", code);
                    } else {
                        let message = url
                            .query_pairs()
                            .find_map(|(key, value)| {
                                (key == "error_description").then(|| value.into_owned())
                            })
                            .unwrap_or_else(|| {
                                String::from("A Microsoft não retornou um código de autorização.")
                            });
                        let _ = app_for_result.emit("microsoft-auth-error", message);
                    }
                }
                Err(_) => {
                    let _ = app_for_result.emit(
                        "microsoft-auth-error",
                        String::from("A Microsoft retornou uma resposta de login inválida."),
                    );
                }
            },
            Err(_) => {
                let _ = app_for_result.emit(
                    "microsoft-auth-error",
                    String::from("Login Microsoft cancelado."),
                );
            }
        }
    });
    Ok(())
}

#[cfg(not(windows))]
#[tauri::command]
fn begin_microsoft_login(app: tauri::AppHandle) -> Result<(), String> {
    record_microsoft_auth_diagnostic("Abrindo a autenticação Microsoft no Linux.");
    if let Some(existing) = app.get_webview_window("microsoft-login") {
        let _ = existing.close();
    }
    let url = reqwest::Url::parse(&microsoft_login_url()).map_err(|error| error.to_string())?;
    let navigation_app = app.clone();
    tauri::WebviewWindowBuilder::new(&app, "microsoft-login", tauri::WebviewUrl::External(url))
        .title("Entrar com a Microsoft — VEX Launcher")
        .inner_size(520.0, 720.0)
        .min_inner_size(420.0, 560.0)
        .center()
        .on_navigation(move |url| {
            let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
            let is_desktop_redirect =
                host == "login.live.com" && url.path().eq_ignore_ascii_case("/oauth20_desktop.srf");
            if is_desktop_redirect {
                let parameters: HashMap<String, String> = url.query_pairs().into_owned().collect();
                if let Some(code) = parameters.get("code") {
                    record_microsoft_auth_diagnostic(
                        "Autorização recebida; iniciando validação da conta.",
                    );
                    let _ = navigation_app.emit("microsoft-auth-code", code.clone());
                } else {
                    let detail = parameters
                        .get("error_description")
                        .or_else(|| parameters.get("error"))
                        .cloned()
                        .unwrap_or_else(|| {
                            String::from("A Microsoft não concluiu a autenticação.")
                        });
                    record_microsoft_auth_diagnostic(&format!(
                        "A Microsoft recusou ou cancelou a autenticação: {detail}"
                    ));
                    let _ = navigation_app.emit("microsoft-auth-error", detail);
                }
                if let Some(window) = navigation_app.get_webview_window("microsoft-login") {
                    let _ = window.close();
                }
                return false;
            }
            url.scheme() == "about"
                || host == "login.live.com"
                || host.ends_with(".live.com")
                || host == "microsoft.com"
                || host.ends_with(".microsoft.com")
                || host == "microsoftonline.com"
                || host.ends_with(".microsoftonline.com")
        })
        .build()
        .map_err(|error| {
            let message = format!("Não foi possível abrir o login Microsoft no Linux: {error}");
            record_microsoft_auth_diagnostic(&message);
            message
        })?;
    Ok(())
}

#[tauri::command]
async fn complete_microsoft_login(code: String) -> Result<MicrosoftAccountStatus, String> {
    let result = async {
        let client = reqwest::Client::builder()
            .user_agent(VEX_USER_AGENT)
            .build()
            .map_err(|error| error.to_string())?;
        let (microsoft_access_token, refresh_token) =
            microsoft_token_from_code(&client, code.trim()).await?;
        let account =
            minecraft_account_from_microsoft_token(&client, &microsoft_access_token, refresh_token)
                .await?;
        write_microsoft_account(&account)?;
        let _ = cache_microsoft_skin(&client, account.skin_url.as_deref()).await;
        let mut settings = read_settings();
        settings.use_offline_profile = false;
        settings.onboarding_completed = true;
        write_settings(&settings)?;
        Ok(microsoft_account_status_value())
    }
    .await;
    match &result {
        Ok(account) => record_microsoft_auth_diagnostic(&format!(
            "Autenticação concluída para a conta {}.",
            account.username
        )),
        Err(error) => record_microsoft_auth_diagnostic(&format!(
            "Falha ao concluir a autenticação Microsoft: {error}"
        )),
    }
    result
}

#[tauri::command]
fn choose_offline_mode() -> Result<LauncherSettings, String> {
    let mut settings = read_settings();
    settings.use_offline_profile = true;
    settings.onboarding_completed = true;
    write_settings(&settings)?;
    Ok(settings)
}

#[tauri::command]
fn use_microsoft_account() -> Result<MicrosoftAccountStatus, String> {
    if read_microsoft_account().is_none() {
        return Err(String::from("Entre com a Microsoft primeiro."));
    }
    let mut settings = read_settings();
    settings.use_offline_profile = false;
    settings.onboarding_completed = true;
    write_settings(&settings)?;
    Ok(microsoft_account_status_value())
}

#[tauri::command]
fn logout_microsoft_account() -> Result<MicrosoftAccountStatus, String> {
    #[cfg(target_os = "linux")]
    delete_linux_microsoft_secret()?;
    let path = microsoft_account_path();
    if path.is_file() {
        fs::remove_file(path).map_err(|error| error.to_string())?;
    }
    let skin_path = microsoft_skin_path();
    if skin_path.is_file() {
        fs::remove_file(skin_path).map_err(|error| error.to_string())?;
    }
    let mut settings = read_settings();
    settings.use_offline_profile = true;
    settings.onboarding_completed = true;
    write_settings(&settings)?;
    Ok(microsoft_account_status_value())
}

#[tauri::command]
fn minimize_window(window: tauri::Window) -> Result<(), String> {
    window.minimize().map_err(|error| error.to_string())
}

#[tauri::command]
fn hide_window_to_tray(window: tauri::Window) -> Result<(), String> {
    window.hide().map_err(|error| error.to_string())
}

#[tauri::command]
fn toggle_maximize_window(window: tauri::Window) -> Result<(), String> {
    if window.is_maximized().map_err(|error| error.to_string())? {
        window.unmaximize().map_err(|error| error.to_string())
    } else {
        window.maximize().map_err(|error| error.to_string())
    }
}

#[tauri::command]
fn close_window(window: tauri::Window) -> Result<(), String> {
    if TRAY_AVAILABLE.load(Ordering::Acquire) {
        window.hide().map_err(|error| error.to_string())
    } else {
        window.minimize().map_err(|error| error.to_string())
    }
}

#[tauri::command]
fn start_window_dragging(window: tauri::Window) -> Result<(), String> {
    window.start_dragging().map_err(|error| error.to_string())
}

#[tauri::command]
fn clear_launcher_cache() -> Result<u64, String> {
    let root = storage_root();
    let mut released = 0;
    for name in ["cache", "downloads", "temp"] {
        let path = root.join(name);
        if path.starts_with(&root) && path.is_dir() {
            released += directory_size(&path);
            fs::remove_dir_all(&path).map_err(|error| error.to_string())?;
        }
        fs::create_dir_all(&path).map_err(|error| error.to_string())?;
    }
    Ok(released)
}

#[tauri::command]
fn read_image_data_url(path: String) -> Result<String, String> {
    let settings = read_settings();
    let target = fs::canonicalize(path).map_err(|error| error.to_string())?;
    let launcher_root = fs::canonicalize(storage_root()).unwrap_or_else(|_| storage_root());
    let game_dir = fs::canonicalize(&settings.game_directory)
        .unwrap_or_else(|_| PathBuf::from(&settings.game_directory));
    if !target.starts_with(&launcher_root) && !target.starts_with(&game_dir) {
        return Err(String::from(
            "A imagem precisa pertencer ao launcher ou à pasta do Minecraft.",
        ));
    }
    let metadata = fs::metadata(&target).map_err(|error| error.to_string())?;
    if !metadata.is_file() || metadata.len() > 10 * 1024 * 1024 {
        return Err(String::from("Imagem inválida ou maior que 10 MB."));
    }
    let mime = match target
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        _ => return Err(String::from("Formato de imagem não suportado.")),
    };
    let bytes = fs::read(target).map_err(|error| error.to_string())?;
    Ok(format!(
        "data:{mime};base64,{}",
        BASE64_STANDARD.encode(bytes)
    ))
}

fn directory_size(path: &Path) -> u64 {
    let mut total = 0;
    if let Ok(entries) = fs::read_dir(path) {
        for entry in entries.flatten() {
            let child = entry.path();
            if child.is_dir() {
                total += directory_size(&child);
            } else if let Ok(metadata) = entry.metadata() {
                total += metadata.len();
            }
        }
    }
    total
}

fn validate_modrinth_download_url(url: &str) -> Result<(), String> {
    let parsed = reqwest::Url::parse(url).map_err(|_| String::from("URL de download inválida."))?;
    let host = parsed.host_str().unwrap_or_default().to_ascii_lowercase();
    if parsed.scheme() != "https"
        || !(host == "cdn.modrinth.com" || host.ends_with(".modrinth.com"))
    {
        return Err(String::from(
            "Por segurança, o conteúdo precisa vir da rede oficial do Modrinth.",
        ));
    }
    Ok(())
}

fn validate_curseforge_download_url(url: &str) -> Result<(), String> {
    let parsed = reqwest::Url::parse(url).map_err(|_| String::from("URL de download inválida."))?;
    let host = parsed.host_str().unwrap_or_default().to_ascii_lowercase();
    if parsed.scheme() != "https"
        || !(host == "edge.forgecdn.net"
            || host == "mediafilez.forgecdn.net"
            || host.ends_with(".forgecdn.net"))
    {
        return Err(String::from(
            "Por segurança, o conteúdo precisa vir da rede oficial do CurseForge.",
        ));
    }
    Ok(())
}

fn modified_unix(path: &Path) -> u64 {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

fn collect_installed_instances(
    root: &Path,
    default_loader: &str,
    default_kind: &str,
    instances: &mut Vec<InstalledInstance>,
) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten().filter(|entry| entry.path().is_dir()) {
        let dir = entry.path();
        let Ok(raw) = fs::read_to_string(dir.join("instance.json")) else {
            continue;
        };
        let Ok(json) = serde_json::from_str::<Value>(&raw) else {
            continue;
        };
        let directory_name = entry.file_name().to_string_lossy().into_owned();
        let version_id = json
            .get("VersionId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        instances.push(InstalledInstance {
            id: json
                .get("Id")
                .and_then(Value::as_str)
                .unwrap_or(&directory_name)
                .to_owned(),
            name: json
                .get("Name")
                .and_then(Value::as_str)
                .unwrap_or(&directory_name)
                .to_owned(),
            loader: json
                .get("Loader")
                .and_then(Value::as_str)
                .unwrap_or(default_loader)
                .to_owned(),
            mc_version: json
                .get("McVersion")
                .and_then(Value::as_str)
                .unwrap_or(&version_id)
                .to_owned(),
            version_id,
            profile_dir: dir.to_string_lossy().into_owned(),
            icon_path: json
                .get("IconPath")
                .and_then(Value::as_str)
                .filter(|path| Path::new(path).exists())
                .map(str::to_owned),
            kind: json
                .get("Kind")
                .and_then(Value::as_str)
                .unwrap_or(default_kind)
                .to_owned(),
            size_mb: (directory_size(&dir) as f64 / 1_048_576.0 * 10.0).round() / 10.0,
            modified_unix: modified_unix(&dir),
            last_played_unix: json
                .get("LastPlayedUnix")
                .and_then(Value::as_u64)
                .unwrap_or_default(),
        });
    }
}

#[tauri::command]
fn list_installed_instances() -> Vec<InstalledInstance> {
    let settings = read_settings();
    let game_dir = PathBuf::from(&settings.game_directory);
    let mut instances = Vec::new();
    collect_installed_instances(
        &game_dir.join("modpacks"),
        "modpack",
        "modpack",
        &mut instances,
    );
    collect_installed_instances(
        &game_dir.join("instances"),
        "vanilla",
        "instance",
        &mut instances,
    );
    let server_instances = storage_root().join("instances");
    if server_instances != game_dir.join("instances") {
        collect_installed_instances(
            &server_instances,
            "vanilla",
            "server-instance",
            &mut instances,
        );
    }

    instances.sort_by(|left, right| {
        right
            .last_played_unix
            .max(right.modified_unix)
            .cmp(&left.last_played_unix.max(left.modified_unix))
    });
    instances
}

async fn resolve_instance_version_id(
    app: &tauri::AppHandle,
    game_dir: &Path,
    minecraft: &str,
    loader: &str,
    operation: &str,
) -> Result<String, String> {
    if loader == "fabric" {
        let client = reqwest::Client::builder()
            .user_agent(VEX_USER_AGENT)
            .build()
            .map_err(|error| error.to_string())?;
        let loaders: Value = client
            .get(format!(
                "https://meta.fabricmc.net/v2/versions/loader/{minecraft}"
            ))
            .send()
            .await
            .map_err(|error| error.to_string())?
            .error_for_status()
            .map_err(|error| error.to_string())?
            .json()
            .await
            .map_err(|error| error.to_string())?;
        let loader_version = loaders
            .as_array()
            .and_then(|items| {
                items
                    .iter()
                    .find(|item| {
                        item.pointer("/loader/stable").and_then(Value::as_bool) == Some(true)
                    })
                    .or_else(|| items.first())
            })
            .and_then(|item| item.pointer("/loader/version"))
            .and_then(Value::as_str)
            .ok_or_else(|| format!("Fabric não está disponível para Minecraft {minecraft}."))?;
        install_fabric_profile(&client, game_dir, minecraft, loader_version).await
    } else if loader == "quilt" {
        let client = reqwest::Client::builder()
            .user_agent(VEX_USER_AGENT)
            .build()
            .map_err(|error| error.to_string())?;
        install_quilt_profile(&client, game_dir, minecraft).await
    } else if loader == "forge" || loader == "neoforge" {
        install_vex_loader_profile(app, game_dir, minecraft, loader, None, operation, 8, 94).await
    } else {
        Ok(minecraft.to_owned())
    }
}

#[tauri::command]
async fn create_instance(
    app: tauri::AppHandle,
    name: String,
    version: String,
    loader: String,
) -> Result<InstalledInstance, String> {
    let clean_name = safe_directory_name(name.trim());
    let clean_version = version.trim();
    let clean_loader = loader.trim().to_lowercase();
    if clean_name.is_empty() || clean_version.is_empty() {
        return Err(String::from("Informe um nome e uma versão."));
    }
    if !["vanilla", "fabric", "quilt", "forge", "neoforge"].contains(&clean_loader.as_str()) {
        return Err(String::from("Loader desconhecido."));
    }
    let settings = read_settings();
    let game_dir = PathBuf::from(&settings.game_directory);
    let instance_dir = game_dir.join("instances").join(&clean_name);
    fs::create_dir_all(&instance_dir).map_err(|error| error.to_string())?;
    let version_id = resolve_instance_version_id(
        &app,
        &game_dir,
        clean_version,
        &clean_loader,
        "create-instance",
    )
    .await?;
    let metadata = serde_json::json!({
        "Id": format!("local-{}", clean_name.to_lowercase().replace(' ', "-")),
        "Name": clean_name,
        "McVersion": clean_version,
        "Loader": clean_loader.clone(),
        "VersionId": version_id.clone(),
        "Source": "VEX Launcher"
    });
    fs::write(
        instance_dir.join("instance.json"),
        serde_json::to_string_pretty(&metadata).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    Ok(InstalledInstance {
        id: metadata["Id"].as_str().unwrap_or_default().to_owned(),
        name: metadata["Name"].as_str().unwrap_or_default().to_owned(),
        loader: clean_loader,
        mc_version: clean_version.to_owned(),
        version_id,
        profile_dir: instance_dir.to_string_lossy().into_owned(),
        icon_path: None,
        kind: String::from("instance"),
        size_mb: 0.0,
        modified_unix: modified_unix(&instance_dir),
        last_played_unix: 0,
    })
}

fn content_folder(category: &str) -> Option<&'static str> {
    match category {
        "Conteúdo" => Some("mods"),
        "Mundos" => Some("saves"),
        "Capturas" => Some("screenshots"),
        "Logs" => Some("logs"),
        _ => None,
    }
}

fn allowed_instance_roots() -> Vec<PathBuf> {
    let settings = read_settings();
    let game_dir = fs::canonicalize(&settings.game_directory)
        .unwrap_or_else(|_| PathBuf::from(&settings.game_directory));
    let server_instances = fs::canonicalize(storage_root().join("instances"))
        .unwrap_or_else(|_| storage_root().join("instances"));
    vec![game_dir, server_instances]
}

fn is_inside_allowed_instance_root(path: &Path) -> bool {
    allowed_instance_roots()
        .iter()
        .any(|root| path.starts_with(root) && path != root)
}

fn containing_managed_instance(path: &Path) -> Option<PathBuf> {
    path.ancestors()
        .skip(1)
        .find(|ancestor| {
            is_inside_allowed_instance_root(ancestor) && ancestor.join("instance.json").is_file()
        })
        .map(Path::to_path_buf)
}

#[tauri::command]
fn list_instance_content(
    profile_dir: String,
    category: String,
) -> Result<Vec<InstanceContent>, String> {
    let profile = fs::canonicalize(profile_dir).map_err(|error| error.to_string())?;
    if !is_inside_allowed_instance_root(&profile) || !profile.join("instance.json").is_file() {
        return Err(String::from(
            "A instância não pertence a uma pasta gerenciada pelo VEX.",
        ));
    }
    let folders: Vec<&str> = if category == "Conteúdo" {
        vec!["mods", "resourcepacks", "shaderpacks"]
    } else {
        vec![content_folder(&category).ok_or_else(|| String::from("Categoria inválida."))?]
    };
    let mut content = Vec::new();
    for folder in folders {
        let root = profile.join(folder);
        fs::create_dir_all(&root).map_err(|error| error.to_string())?;
        for entry in fs::read_dir(&root)
            .map_err(|error| error.to_string())?
            .flatten()
        {
            let path = entry.path();
            let size = if path.is_dir() {
                directory_size(&path)
            } else {
                entry
                    .metadata()
                    .map(|metadata| metadata.len())
                    .unwrap_or_default()
            };
            let kind = match folder {
                "mods" => "Mod",
                "resourcepacks" => "Textura",
                "shaderpacks" => "Shader",
                _ if path.is_dir() => "Pasta",
                _ => path
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .unwrap_or("Arquivo"),
            };
            content.push(InstanceContent {
                name: entry.file_name().to_string_lossy().into_owned(),
                path: path.to_string_lossy().into_owned(),
                kind: kind.to_owned(),
                size_mb: (size as f64 / 1_048_576.0 * 10.0).round() / 10.0,
                modified_unix: modified_unix(&path),
            });
        }
    }
    content.sort_by_key(|entry| std::cmp::Reverse(entry.modified_unix));
    Ok(content)
}

#[tauri::command]
fn remove_instance_content(path: String) -> Result<(), String> {
    let target = fs::canonicalize(path).map_err(|error| error.to_string())?;
    if containing_managed_instance(&target).is_none() {
        return Err(String::from(
            "O arquivo precisa ficar dentro do conteúdo de uma instância gerenciada.",
        ));
    }
    if target.is_dir() {
        fs::remove_dir_all(target).map_err(|error| error.to_string())
    } else {
        fs::remove_file(target).map_err(|error| error.to_string())
    }
}

fn copy_directory(source: &Path, destination: &Path) -> Result<(), String> {
    fs::create_dir_all(destination).map_err(|error| error.to_string())?;
    for entry in fs::read_dir(source).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let target = destination.join(entry.file_name());
        if entry.path().is_dir() {
            copy_directory(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), target).map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

fn count_directory_entries(path: &Path, directories_only: bool) -> usize {
    fs::read_dir(path)
        .map(|entries| {
            entries
                .flatten()
                .filter(|entry| !directories_only || entry.path().is_dir())
                .count()
        })
        .unwrap_or_default()
}

fn minecraft_installation_scan(path: &Path, game_dir: &Path) -> Option<MinecraftInstallationScan> {
    if !path.is_dir() {
        return None;
    }
    let versions_dir = path.join("versions");
    let mods = count_directory_entries(&path.join("mods"), false);
    let worlds = count_directory_entries(&path.join("saves"), true);
    let resource_packs = count_directory_entries(&path.join("resourcepacks"), false);
    let shaders = count_directory_entries(&path.join("shaderpacks"), false);
    let mut versions: Vec<String> = fs::read_dir(&versions_dir)
        .map(|entries| {
            entries
                .flatten()
                .filter(|entry| entry.path().is_dir())
                .filter_map(|entry| entry.file_name().to_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    let instance_metadata = fs::read_to_string(path.join("instance.json"))
        .ok()
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok());
    if let Some(version) = instance_metadata
        .as_ref()
        .and_then(|metadata| metadata.get("McVersion"))
        .and_then(Value::as_str)
    {
        versions.push(version.to_owned());
    }
    versions.sort_by(|left, right| right.cmp(left));
    versions.dedup();
    versions.truncate(8);
    if versions.is_empty()
        && mods == 0
        && worlds == 0
        && resource_packs == 0
        && shaders == 0
        && !path.join("instance.json").is_file()
    {
        return None;
    }
    let mut loaders = Vec::new();
    for version in &versions {
        let lower = version.to_ascii_lowercase();
        for loader in ["fabric", "forge", "neoforge", "quilt", "vanilla"] {
            if lower.contains(loader) && !loaders.iter().any(|item| item == loader) {
                loaders.push(loader.to_owned());
            }
        }
    }
    if let Some(loader) = instance_metadata
        .as_ref()
        .and_then(|metadata| metadata.get("Loader"))
        .and_then(Value::as_str)
    {
        if !loaders.iter().any(|item| item.eq_ignore_ascii_case(loader)) {
            loaders.push(loader.to_owned());
        }
    }
    if loaders.is_empty() && !versions.is_empty() {
        loaders.push(String::from("vanilla"));
    }
    let canonical_game = fs::canonicalize(game_dir).unwrap_or_else(|_| game_dir.to_path_buf());
    let canonical_path = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let managed_by_vex = path.join("instance.json").is_file()
        || canonical_path.starts_with(canonical_game.join("instances"))
        || canonical_path.starts_with(canonical_game.join("modpacks"));
    Some(MinecraftInstallationScan {
        path: path.to_string_lossy().into_owned(),
        label: path
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .unwrap_or("Minecraft")
            .to_owned(),
        versions,
        loaders,
        mods,
        worlds,
        resource_packs,
        shaders,
        size_mb: (directory_size(path) as f64 / 1_048_576.0 * 10.0).round() / 10.0,
        managed_by_vex,
    })
}

fn add_candidate(candidates: &mut HashSet<PathBuf>, path: PathBuf) {
    if path.is_dir() {
        candidates.insert(fs::canonicalize(&path).unwrap_or(path));
    }
}

#[tauri::command]
fn scan_minecraft_installations() -> Vec<MinecraftInstallationScan> {
    let settings = read_settings();
    let game_dir = PathBuf::from(&settings.game_directory);
    let mut candidates = HashSet::new();
    add_candidate(&mut candidates, game_dir.clone());
    add_candidate(&mut candidates, default_game_directory());
    if Path::new(r"D:\").is_dir() {
        add_candidate(&mut candidates, PathBuf::from(r"D:\.minecraft"));
    }
    if let Ok(appdata) = env::var("APPDATA") {
        let appdata = PathBuf::from(appdata);
        add_candidate(&mut candidates, appdata.join(".minecraft"));
        for root in [
            appdata
                .join("CurseForge")
                .join("Minecraft")
                .join("Instances"),
            appdata.join("ModrinthApp").join("profiles"),
        ] {
            if let Ok(entries) = fs::read_dir(root) {
                for entry in entries.flatten().filter(|entry| entry.path().is_dir()) {
                    add_candidate(&mut candidates, entry.path());
                }
            }
        }
    }
    if let Ok(home) = env::var("HOME") {
        let home = PathBuf::from(home);
        add_candidate(&mut candidates, home.join(".minecraft"));
        for root in [
            home.join(".local")
                .join("share")
                .join("ModrinthApp")
                .join("profiles"),
            home.join(".local")
                .join("share")
                .join("PrismLauncher")
                .join("instances"),
        ] {
            if let Ok(entries) = fs::read_dir(root) {
                for entry in entries.flatten().filter(|entry| entry.path().is_dir()) {
                    add_candidate(&mut candidates, entry.path());
                }
            }
        }
    }
    for root in [game_dir.join("instances"), game_dir.join("modpacks")] {
        if let Ok(entries) = fs::read_dir(root) {
            for entry in entries.flatten().filter(|entry| entry.path().is_dir()) {
                add_candidate(&mut candidates, entry.path());
            }
        }
    }
    let mut scans: Vec<MinecraftInstallationScan> = candidates
        .into_iter()
        .filter_map(|path| minecraft_installation_scan(&path, &game_dir))
        .collect();
    scans.sort_by(|left, right| {
        right
            .managed_by_vex
            .cmp(&left.managed_by_vex)
            .then_with(|| right.size_mb.total_cmp(&left.size_mb))
    });
    scans
}

fn backup_instance_directory(source: &Path) -> Result<PathBuf, String> {
    let base_name = source
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("instance");
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default();
    let destination = storage_root()
        .join("backups")
        .join("instances")
        .join(format!("{}-{timestamp}", safe_directory_name(base_name)));
    copy_directory(source, &destination)?;
    Ok(destination)
}

#[tauri::command]
fn backup_instance(profile_dir: String) -> Result<String, String> {
    let source = validated_instance_dir(&profile_dir)?;
    backup_instance_directory(&source).map(|path| path.to_string_lossy().into_owned())
}

fn validated_instance_dir(profile_dir: &str) -> Result<PathBuf, String> {
    let target = fs::canonicalize(profile_dir).map_err(|error| error.to_string())?;
    if !is_inside_allowed_instance_root(&target) || !target.join("instance.json").is_file() {
        return Err(String::from(
            "Instância inválida ou fora das pastas gerenciadas pelo VEX.",
        ));
    }
    Ok(target)
}

#[tauri::command]
fn delete_instance(profile_dir: String, confirmation: String) -> Result<(), String> {
    if confirmation.trim() != "SIM" && confirmation.trim() != "YES" {
        return Err(String::from(
            "Digite SIM ou YES em letras maiúsculas para confirmar.",
        ));
    }
    let target = validated_instance_dir(&profile_dir)?;
    fs::remove_dir_all(target).map_err(|error| error.to_string())
}

#[tauri::command]
fn clone_instance(profile_dir: String) -> Result<String, String> {
    let source = validated_instance_dir(&profile_dir)?;
    let base_name = source
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("instancia");
    let parent = source
        .parent()
        .ok_or_else(|| String::from("Pasta da instância inválida."))?;
    let mut index = 1;
    let destination = loop {
        let suffix = if index == 1 {
            String::from(" - Copia")
        } else {
            format!(" - Copia {index}")
        };
        let candidate = parent.join(format!("{base_name}{suffix}"));
        if !candidate.exists() {
            break candidate;
        }
        index += 1;
    };
    copy_directory(&source, &destination)?;
    let metadata_path = destination.join("instance.json");
    if let Ok(raw) = fs::read_to_string(&metadata_path) {
        if let Ok(mut metadata) = serde_json::from_str::<Value>(&raw) {
            let original_name = metadata
                .get("Name")
                .and_then(Value::as_str)
                .unwrap_or(base_name)
                .to_owned();
            metadata["Name"] = Value::String(format!("{original_name} - Cópia"));
            metadata["Id"] = Value::String(format!(
                "local-{}",
                safe_directory_name(&format!("{original_name}-copia"))
                    .to_lowercase()
                    .replace(' ', "-")
            ));
            metadata["LastPlayedUnix"] = Value::from(0_u64);
            fs::write(
                metadata_path,
                serde_json::to_string_pretty(&metadata).map_err(|error| error.to_string())?,
            )
            .map_err(|error| error.to_string())?;
        }
    }
    Ok(destination.to_string_lossy().into_owned())
}

#[tauri::command]
fn set_instance_icon(profile_dir: String, bytes: Vec<u8>) -> Result<String, String> {
    let target = validated_instance_dir(&profile_dir)?;
    if bytes.len() > 8 * 1024 * 1024 || !bytes.starts_with(&[137, 80, 78, 71, 13, 10, 26, 10]) {
        return Err(String::from("Use uma imagem PNG de até 8 MB."));
    }
    let icon_path = target.join("icon.png");
    fs::write(&icon_path, bytes).map_err(|error| error.to_string())?;
    let metadata_path = target.join("instance.json");
    let mut metadata: Value = serde_json::from_str(
        &fs::read_to_string(&metadata_path).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    metadata["IconPath"] = Value::String(icon_path.to_string_lossy().into_owned());
    fs::write(
        metadata_path,
        serde_json::to_string_pretty(&metadata).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    Ok(icon_path.to_string_lossy().into_owned())
}

fn collect_java_in_dir(root: &Path, depth: usize, output: &mut Vec<PathBuf>) {
    if depth == 0 || !root.is_dir() {
        return;
    }
    #[cfg(windows)]
    let candidates = [
        root.join("bin").join("java.exe"),
        root.join("bin").join("javaw.exe"),
        root.join("java.exe"),
        root.join("javaw.exe"),
    ];
    #[cfg(not(windows))]
    let candidates = [
        root.join("bin").join("java"),
        root.join("bin").join("java"),
        root.join("java"),
        root.join("java"),
    ];
    for candidate in candidates {
        if candidate.is_file() {
            output.push(candidate);
        }
    }
    if let Ok(entries) = fs::read_dir(root) {
        for entry in entries.flatten().filter(|entry| entry.path().is_dir()) {
            collect_java_in_dir(&entry.path(), depth - 1, output);
        }
    }
}

fn java_major(path: &Path) -> u32 {
    let java = if path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("javaw.exe"))
    {
        path.with_file_name("java.exe")
    } else {
        path.to_owned()
    };
    let Ok(output) = hidden_command(java).arg("-version").output() else {
        return 0;
    };
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    let Some(start) = text.find("version \"").map(|index| index + 9) else {
        return 0;
    };
    let version = text[start..].split('"').next().unwrap_or_default();
    let parts: Vec<&str> = version.split('.').collect();
    if parts.first() == Some(&"1") {
        parts
            .get(1)
            .and_then(|value| value.parse().ok())
            .unwrap_or(0)
    } else {
        parts
            .first()
            .and_then(|value| value.parse().ok())
            .unwrap_or(0)
    }
}

#[tauri::command]
fn detect_java_runtimes() -> Vec<JavaRuntime> {
    let settings = read_settings();
    let mut candidates = Vec::new();
    if let Ok(java_home) = env::var("JAVA_HOME") {
        collect_java_in_dir(Path::new(&java_home), 2, &mut candidates);
    }
    collect_java_in_dir(
        &PathBuf::from(&settings.game_directory).join("runtime"),
        6,
        &mut candidates,
    );
    collect_java_in_dir(&storage_root().join("runtimes"), 7, &mut candidates);
    for variable in ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432"] {
        if let Ok(root) = env::var(variable) {
            for vendor in [
                "Java",
                "Eclipse Adoptium",
                "Microsoft",
                "Azul Systems",
                "Amazon Corretto",
                "BellSoft",
            ] {
                collect_java_in_dir(&PathBuf::from(&root).join(vendor), 3, &mut candidates);
            }
        }
    }
    if let Some(path) = env::var_os("PATH") {
        for dir in env::split_paths(&path) {
            #[cfg(windows)]
            let filenames = ["java.exe", "javaw.exe"];
            #[cfg(not(windows))]
            let filenames = ["java", "java"];
            for filename in filenames {
                let candidate = dir.join(filename);
                if candidate.is_file() {
                    candidates.push(candidate);
                }
            }
        }
    }

    let mut seen = HashSet::new();
    let mut runtimes: Vec<JavaRuntime> = candidates
        .into_iter()
        .filter_map(|path| {
            let canonical = fs::canonicalize(&path).unwrap_or(path);
            if !seen.insert(canonical.clone()) {
                return None;
            }
            let major = java_major(&canonical);
            (major > 0).then(|| JavaRuntime {
                path: canonical.to_string_lossy().into_owned(),
                major,
            })
        })
        .collect();
    runtimes.sort_by_key(|runtime| std::cmp::Reverse(runtime.major));
    runtimes
}

async fn ensure_java_runtime(
    app: &tauri::AppHandle,
    required_java: u32,
    operation: &str,
    start: u8,
    end: u8,
) -> Result<JavaRuntime, String> {
    let mut runtimes = detect_java_runtimes();
    runtimes.sort_by_key(|runtime| runtime.major);
    if let Some(runtime) = runtimes
        .iter()
        .find(|runtime| runtime.major == required_java)
        .cloned()
    {
        return Ok(runtime);
    }

    emit_progress(
        app,
        operation,
        format!("Java {required_java} não encontrado. Preparando download automático"),
        start,
        false,
    );
    let client = reqwest::Client::builder()
        .user_agent(VEX_USER_AGENT)
        .build()
        .map_err(|error| error.to_string())?;
    #[cfg(windows)]
    let platform = "windows";
    #[cfg(target_os = "linux")]
    let platform = "linux";
    #[cfg(target_os = "macos")]
    let platform = "mac";
    let assets: Value = client
        .get(format!(
            "https://api.adoptium.net/v3/assets/latest/{required_java}/hotspot?architecture=x64&image_type=jre&os={platform}&vendor=eclipse"
        ))
        .send()
        .await
        .map_err(|error| error.to_string())?
        .error_for_status()
        .map_err(|error| error.to_string())?
        .json()
        .await
        .map_err(|error| error.to_string())?;
    let package = assets
        .as_array()
        .and_then(|items| items.first())
        .and_then(|asset| asset.get("binary"))
        .and_then(|binary| binary.get("package"))
        .ok_or_else(|| {
            format!("Não foi possível localizar o Java {required_java} para este sistema.")
        })?;
    let download_url = package.get("link").and_then(Value::as_str).ok_or_else(|| {
        format!("Não foi possível localizar o Java {required_java} para este sistema.")
    })?;
    let expected_checksum = package
        .get("checksum")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            String::from("O fornecedor do Java não informou a verificação de integridade.")
        })?;
    let download_end = end.saturating_sub(8).max(start);
    let bytes = download_bytes_with_progress(
        app,
        operation,
        &format!("Baixando Java {required_java}"),
        &client,
        download_url,
        start.saturating_add(2),
        download_end,
    )
    .await?;
    let actual_checksum = format!("{:x}", Sha256::digest(&bytes));
    if !actual_checksum.eq_ignore_ascii_case(expected_checksum) {
        return Err(String::from(
            "O download do Java não passou na verificação de integridade SHA-256.",
        ));
    }

    emit_progress(
        app,
        operation,
        format!("Instalando Java {required_java} na pasta protegida do VEX"),
        download_end.saturating_add(1),
        false,
    );
    let runtimes_root = storage_root().join("runtimes");
    let destination = runtimes_root.join(format!("temurin-{required_java}"));
    let staging = runtimes_root.join(format!(".temurin-{required_java}-installing"));
    fs::create_dir_all(&runtimes_root).map_err(|error| error.to_string())?;
    if staging.is_dir() {
        fs::remove_dir_all(&staging).map_err(|error| error.to_string())?;
    }
    fs::create_dir_all(&staging).map_err(|error| error.to_string())?;
    #[cfg(windows)]
    {
        let mut archive =
            zip::ZipArchive::new(Cursor::new(bytes)).map_err(|error| error.to_string())?;
        let archive_len = archive.len().max(1);
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index).map_err(|error| error.to_string())?;
            let Some(relative) = entry.enclosed_name() else {
                continue;
            };
            let output = staging.join(relative);
            if entry.is_dir() {
                fs::create_dir_all(&output).map_err(|error| error.to_string())?;
            } else {
                if let Some(parent) = output.parent() {
                    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
                }
                let mut file = fs::File::create(output).map_err(|error| error.to_string())?;
                std::io::copy(&mut entry, &mut file).map_err(|error| error.to_string())?;
            }
            let percent = download_end.saturating_add(
                (((index + 1) as f64 / archive_len as f64) * f64::from(end - download_end)) as u8,
            );
            emit_progress(
                app,
                operation,
                format!("Instalando Java {required_java}"),
                percent,
                false,
            );
        }
    }
    #[cfg(not(windows))]
    {
        let decoder = flate2::read::GzDecoder::new(Cursor::new(bytes));
        let mut archive = tar::Archive::new(decoder);
        archive
            .unpack(&staging)
            .map_err(|error| error.to_string())?;
        emit_progress(
            app,
            operation,
            format!("Instalando Java {required_java}"),
            end.saturating_sub(1),
            false,
        );
    }
    if destination.is_dir() {
        fs::remove_dir_all(&destination).map_err(|error| error.to_string())?;
    }
    fs::rename(&staging, &destination).map_err(|error| error.to_string())?;

    let mut installed = Vec::new();
    collect_java_in_dir(&destination, 7, &mut installed);
    installed
        .into_iter()
        .find_map(|path| {
            let major = java_major(&path);
            (major == required_java).then(|| JavaRuntime {
                path: path.to_string_lossy().into_owned(),
                major,
            })
        })
        .ok_or_else(|| {
            format!("O Java {required_java} foi baixado, mas o executável não foi encontrado.")
        })
}

fn merge_version_json(mut child: Value, parent: Value) -> Value {
    for key in [
        "mainClass",
        "downloads",
        "assetIndex",
        "javaVersion",
        "minecraftArguments",
        "arguments",
        "type",
    ] {
        let missing =
            child.get(key).is_none() || child.get(key).is_some_and(|value| value.is_null());
        if missing {
            if let Some(value) = parent.get(key) {
                child[key] = value.clone();
            }
        }
    }
    let mut libraries = parent
        .get("libraries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for child_library in child
        .get("libraries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
    {
        let name = child_library
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if let Some(index) = libraries
            .iter()
            .position(|library| library.get("name").and_then(Value::as_str) == Some(name))
        {
            libraries[index] = child_library;
        } else {
            libraries.push(child_library);
        }
    }
    child["libraries"] = Value::Array(libraries);
    child
}

async fn download_to(
    client: &reqwest::Client,
    url: &str,
    destination: &Path,
) -> Result<(), String> {
    if destination.is_file() {
        return Ok(());
    }
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|error| error.to_string())?
        .error_for_status()
        .map_err(|error| error.to_string())?;
    let bytes = response.bytes().await.map_err(|error| error.to_string())?;
    fs::write(destination, bytes).map_err(|error| error.to_string())
}

async fn download_bytes_with_progress(
    app: &tauri::AppHandle,
    operation: &str,
    label: &str,
    client: &reqwest::Client,
    url: &str,
    start: u8,
    end: u8,
) -> Result<Vec<u8>, String> {
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|error| error.to_string())?
        .error_for_status()
        .map_err(|error| error.to_string())?;
    let total = response.content_length();
    let mut downloaded = 0_u64;
    let mut bytes = Vec::with_capacity(total.unwrap_or_default().min(256 * 1024 * 1024) as usize);
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| error.to_string())?;
        downloaded += chunk.len() as u64;
        bytes.extend_from_slice(&chunk);
        let percent = total
            .filter(|value| *value > 0)
            .map(|value| {
                start + (((downloaded as f64 / value as f64) * f64::from(end - start)) as u8)
            })
            .unwrap_or(start);
        emit_progress(app, operation, label, percent.min(end), false);
    }
    emit_progress(app, operation, label, end, false);
    Ok(bytes)
}

async fn get_version_json(
    client: &reqwest::Client,
    game_dir: &Path,
    version_id: &str,
) -> Result<Value, String> {
    let local_path = game_dir
        .join("versions")
        .join(version_id)
        .join(format!("{version_id}.json"));
    let mut json: Value = if local_path.is_file() {
        serde_json::from_str(&fs::read_to_string(&local_path).map_err(|error| error.to_string())?)
            .map_err(|error| error.to_string())?
    } else {
        let manifest: Value = client
            .get("https://launchermeta.mojang.com/mc/game/version_manifest.json")
            .send()
            .await
            .map_err(|error| error.to_string())?
            .json()
            .await
            .map_err(|error| error.to_string())?;
        let url = manifest
            .get("versions")
            .and_then(Value::as_array)
            .and_then(|versions| {
                versions
                    .iter()
                    .find(|version| version.get("id").and_then(Value::as_str) == Some(version_id))
            })
            .and_then(|version| version.get("url"))
            .and_then(Value::as_str)
            .ok_or_else(|| format!("Versão {version_id} não encontrada no manifesto Mojang."))?;
        let downloaded: Value = client
            .get(url)
            .send()
            .await
            .map_err(|error| error.to_string())?
            .json()
            .await
            .map_err(|error| error.to_string())?;
        if let Some(parent) = local_path.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        fs::write(
            &local_path,
            serde_json::to_string_pretty(&downloaded).map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
        downloaded
    };
    if let Some(parent_id) = json
        .get("inheritsFrom")
        .and_then(Value::as_str)
        .map(str::to_owned)
    {
        let parent = Box::pin(get_version_json(client, game_dir, &parent_id)).await?;
        json = merge_version_json(json, parent);
    }
    Ok(json)
}

fn maven_path(coordinate: &str) -> Option<String> {
    let (coordinate, extension) = coordinate.split_once('@').unwrap_or((coordinate, "jar"));
    let parts: Vec<&str> = coordinate.split(':').collect();
    if parts.len() < 3 {
        return None;
    }
    let classifier = parts
        .get(3)
        .map(|value| format!("-{value}"))
        .unwrap_or_default();
    Some(format!(
        "{}/{}/{}/{}-{}{}.{}",
        parts[0].replace('.', "/"),
        parts[1],
        parts[2],
        parts[1],
        parts[2],
        classifier,
        extension
    ))
}

fn current_os_name() -> &'static str {
    #[cfg(target_os = "windows")]
    return "windows";
    #[cfg(target_os = "linux")]
    return "linux";
    #[cfg(target_os = "macos")]
    return "osx";
    #[allow(unreachable_code)]
    "unknown"
}

fn should_use_library(library: &Value) -> bool {
    let Some(rules) = library.get("rules").and_then(Value::as_array) else {
        return true;
    };
    if rules.is_empty() {
        return true;
    }
    let mut allowed = false;
    for rule in rules {
        let action = rule
            .get("action")
            .and_then(Value::as_str)
            .unwrap_or("disallow");
        match rule
            .get("os")
            .and_then(|os| os.get("name"))
            .and_then(Value::as_str)
        {
            None => allowed = action == "allow",
            Some(name) if name == current_os_name() => allowed = action == "allow",
            Some(_) if action == "disallow" => allowed = true,
            _ => {}
        }
    }
    allowed
}

fn argument_rules_allow(argument: &Value) -> bool {
    let Some(rules) = argument.get("rules").and_then(Value::as_array) else {
        return true;
    };
    let mut allowed = false;
    for rule in rules {
        let action = rule
            .get("action")
            .and_then(Value::as_str)
            .unwrap_or("disallow");
        let os_matches = rule
            .get("os")
            .and_then(|os| os.get("name"))
            .and_then(Value::as_str)
            .is_none_or(|name| name == current_os_name());
        let features_match = rule
            .get("features")
            .and_then(Value::as_object)
            .is_none_or(|features| features.is_empty());
        if os_matches && features_match {
            allowed = action == "allow";
        }
    }
    allowed
}

fn version_arguments(version: &Value, kind: &str) -> Vec<String> {
    version
        .get("arguments")
        .and_then(|arguments| arguments.get(kind))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|argument| argument_rules_allow(argument))
        .flat_map(|argument| {
            if let Some(value) = argument.as_str() {
                vec![value.to_owned()]
            } else if let Some(value) = argument.get("value") {
                value
                    .as_array()
                    .map(|values| {
                        values
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect()
                    })
                    .or_else(|| value.as_str().map(|value| vec![value.to_owned()]))
                    .unwrap_or_default()
            } else {
                Vec::new()
            }
        })
        .collect()
}

fn replace_launch_placeholders(value: &str, replacements: &[(&str, &str)]) -> String {
    replacements
        .iter()
        .fold(value.to_owned(), |result, (key, replacement)| {
            result.replace(key, replacement)
        })
}

fn extract_native(archive_path: &Path, destination: &Path) -> Result<(), String> {
    let file = fs::File::open(archive_path).map_err(|error| error.to_string())?;
    let mut archive = zip::ZipArchive::new(file).map_err(|error| error.to_string())?;
    fs::create_dir_all(destination).map_err(|error| error.to_string())?;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|error| error.to_string())?;
        if entry.is_dir() || entry.name().starts_with("META-INF/") {
            continue;
        }
        let Some(relative) = entry.enclosed_name() else {
            continue;
        };
        let output = destination.join(relative);
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let mut file = fs::File::create(output).map_err(|error| error.to_string())?;
        std::io::copy(&mut entry, &mut file).map_err(|error| error.to_string())?;
    }
    Ok(())
}

async fn prepare_libraries(
    client: &reqwest::Client,
    version: &Value,
    game_dir: &Path,
    natives_dir: &Path,
    client_jar: &Path,
) -> Result<Vec<String>, String> {
    let libraries_dir = game_dir.join("libraries");
    let mut classpath = vec![client_jar.to_string_lossy().into_owned()];
    for library in version
        .get("libraries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
    {
        if !should_use_library(&library) {
            continue;
        }
        if let Some(destination) =
            prepare_library_artifact(client, &library, &libraries_dir).await?
        {
            classpath.push(destination.to_string_lossy().into_owned());
        }

        let native_key = library
            .get("natives")
            .and_then(|natives| natives.get(current_os_name()))
            .and_then(Value::as_str)
            .unwrap_or_else(|| match current_os_name() {
                "linux" => "natives-linux",
                "osx" => "natives-osx",
                _ => "natives-windows",
            })
            .replace("${arch}", "64");
        if let Some(native) = library
            .get("downloads")
            .and_then(|downloads| downloads.get("classifiers"))
            .and_then(|classifiers| classifiers.get(&native_key))
        {
            if let (Some(path), Some(url)) = (
                native.get("path").and_then(Value::as_str),
                native.get("url").and_then(Value::as_str),
            ) {
                let destination =
                    libraries_dir.join(path.replace('/', std::path::MAIN_SEPARATOR_STR));
                if download_to(client, url, &destination).await.is_ok() {
                    let _ = extract_native(&destination, natives_dir);
                }
            }
        }
    }
    Ok(classpath)
}

async fn prepare_library_artifact(
    client: &reqwest::Client,
    library: &Value,
    libraries_dir: &Path,
) -> Result<Option<PathBuf>, String> {
    if !should_use_library(library) {
        return Ok(None);
    }
    let Some(name) = library.get("name").and_then(Value::as_str) else {
        return Ok(None);
    };
    let artifact = library
        .get("downloads")
        .and_then(|downloads| downloads.get("artifact"));
    let relative = artifact
        .and_then(|artifact| artifact.get("path"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| maven_path(name))
        .ok_or_else(|| format!("Invalid Maven coordinate: {name}"))?;
    let destination = libraries_dir.join(relative.replace('/', std::path::MAIN_SEPARATOR_STR));
    if destination.is_file() {
        return Ok(Some(destination));
    }
    if let Some(url) = artifact
        .and_then(|artifact| artifact.get("url"))
        .and_then(Value::as_str)
    {
        if download_to(client, url, &destination).await.is_ok() && destination.is_file() {
            return Ok(Some(destination));
        }
    }
    let bases = [
        library.get("url").and_then(Value::as_str).unwrap_or(""),
        "https://maven.fabricmc.net/",
        "https://maven.minecraftforge.net/",
        "https://maven.neoforged.net/releases/",
        "https://libraries.minecraft.net/",
        "https://repo1.maven.org/maven2/",
    ];
    for base in bases.into_iter().filter(|base| !base.is_empty()) {
        let url = format!(
            "{}{}",
            base.trim_end_matches('/').to_owned() + "/",
            relative
        );
        if download_to(client, &url, &destination).await.is_ok() && destination.is_file() {
            return Ok(Some(destination));
        }
    }
    Err(format!("Could not download required library {name}."))
}

async fn prepare_assets(
    client: &reqwest::Client,
    version: &Value,
    game_dir: &Path,
) -> Result<(PathBuf, String), String> {
    let asset_index = version
        .get("assetIndex")
        .ok_or_else(|| String::from("Versão sem índice de assets."))?;
    let index_id = asset_index
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| String::from("Índice de assets sem ID."))?
        .to_owned();
    let index_url = asset_index
        .get("url")
        .and_then(Value::as_str)
        .ok_or_else(|| String::from("Índice de assets sem URL."))?;
    let assets_dir = game_dir.join("assets");
    let index_path = assets_dir.join("indexes").join(format!("{index_id}.json"));
    download_to(client, index_url, &index_path).await?;
    let index: Value =
        serde_json::from_str(&fs::read_to_string(index_path).map_err(|error| error.to_string())?)
            .map_err(|error| error.to_string())?;
    let pending: Vec<(String, PathBuf)> = index
        .get("objects")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|objects| objects.values())
        .filter_map(|object| {
            let hash = object.get("hash").and_then(Value::as_str)?;
            let prefix = hash.get(0..2)?;
            let destination = assets_dir.join("objects").join(prefix).join(hash);
            (!destination.is_file()).then(|| {
                (
                    format!("https://resources.download.minecraft.net/{prefix}/{hash}"),
                    destination,
                )
            })
        })
        .collect();
    for chunk in pending.chunks(24) {
        let futures = chunk
            .iter()
            .map(|(url, path)| download_to(client, url, path));
        let _ = futures::future::join_all(futures).await;
    }
    Ok((assets_dir, index_id))
}

fn apply_offline_skin(skin: &Path, profile_dir: &Path) -> Result<(), String> {
    if !skin.is_file() {
        return Ok(());
    }
    let pack_dir = profile_dir
        .join("resourcepacks")
        .join("launcher_offline_skin");
    let entity_dir = pack_dir
        .join("assets")
        .join("minecraft")
        .join("textures")
        .join("entity");
    for model in [
        "alex", "ari", "efe", "kai", "makena", "noor", "steve", "sunny", "zuri",
    ] {
        for arm in ["wide", "slim"] {
            let destination = entity_dir
                .join("player")
                .join(arm)
                .join(format!("{model}.png"));
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent).map_err(|error| error.to_string())?;
            }
            fs::copy(skin, destination).map_err(|error| error.to_string())?;
        }
    }
    fs::copy(skin, entity_dir.join("steve.png")).map_err(|error| error.to_string())?;
    fs::copy(skin, entity_dir.join("alex.png")).map_err(|error| error.to_string())?;
    fs::write(
        pack_dir.join("pack.mcmeta"),
        r#"{"pack":{"pack_format":34,"description":"Skin offline do VEX Launcher"}}"#,
    )
    .map_err(|error| error.to_string())?;

    let options_path = profile_dir.join("options.txt");
    let mut lines: Vec<String> = fs::read_to_string(&options_path)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect();
    let mut packs = Vec::<String>::new();
    if let Some(existing) = lines.iter().find(|line| line.starts_with("resourcePacks:")) {
        packs =
            serde_json::from_str(existing.trim_start_matches("resourcePacks:")).unwrap_or_default();
    }
    packs.retain(|pack| pack != "file/launcher_offline_skin");
    packs.push(String::from("file/launcher_offline_skin"));
    let replacement = format!(
        "resourcePacks:{}",
        serde_json::to_string(&packs).map_err(|error| error.to_string())?
    );
    if let Some(line) = lines
        .iter_mut()
        .find(|line| line.starts_with("resourcePacks:"))
    {
        *line = replacement;
    } else {
        lines.push(replacement);
    }
    fs::write(options_path, lines.join("\n")).map_err(|error| error.to_string())
}

fn offline_uuid(username: &str) -> String {
    let mut bytes = md5::compute(format!("OfflinePlayer:{username}")).0;
    bytes[6] = (bytes[6] & 0x0f) | 0x30;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

fn spawn_log_reader<R: std::io::Read + Send + 'static>(reader: R, log_path: PathBuf) {
    std::thread::spawn(move || {
        let mut reader = reader;
        let mut buffer = [0_u8; 8192];
        while let Ok(read) = reader.read(&mut buffer) {
            if read == 0 {
                break;
            }
            let chunk = String::from_utf8_lossy(&buffer[..read]);
            write_server_log_chunk(&log_path, chunk.as_bytes());
        }
    });
}

fn append_log(log_path: &Path, message: &str) {
    write_server_log_line(log_path, &format!("[Launcher] {message}"));
}

fn read_text_tail(path: &Path, limit: u64) -> String {
    let Ok(mut file) = fs::File::open(path) else {
        return String::new();
    };
    let length = file.metadata().map(|metadata| metadata.len()).unwrap_or(0);
    let start = length.saturating_sub(limit);
    if file.seek(SeekFrom::Start(start)).is_err() {
        return String::new();
    }
    let mut bytes = Vec::with_capacity(length.saturating_sub(start).min(limit) as usize);
    if file.take(limit).read_to_end(&mut bytes).is_err() {
        return String::new();
    }
    if start > 0 {
        if let Some(newline) = bytes.iter().position(|byte| *byte == b'\n') {
            bytes.drain(..=newline);
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

fn write_server_log_line(log_path: &Path, line: &str) {
    let mut chunk = Vec::with_capacity(line.len().saturating_add(1));
    chunk.extend_from_slice(line.as_bytes());
    chunk.push(b'\n');
    write_server_log_chunk(log_path, &chunk);
}

fn write_server_log_chunk(log_path: &Path, chunk: &[u8]) {
    let _guard = server_log_write_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if fs::metadata(log_path)
        .map(|metadata| metadata.len() >= SERVER_LOG_MAX_BYTES)
        .unwrap_or(false)
    {
        let tail = read_text_tail(log_path, SERVER_LOG_RETAIN_BYTES);
        let _ = fs::write(
            log_path,
            format!("[Launcher] Console compactado para proteger o espaço em disco.\n{tail}"),
        );
    }
    if let Ok(mut log) = OpenOptions::new().create(true).append(true).open(log_path) {
        let _ = log.write_all(chunk);
    }
}

fn curseforge_class_id(project_type: &str) -> Option<u32> {
    match project_type {
        "mod" => Some(6),
        "modpack" => Some(4471),
        "resourcepack" => Some(12),
        "shader" => Some(6552),
        "plugin" => Some(5),
        _ => None,
    }
}

fn curseforge_loader_type(loader: &str) -> Option<u32> {
    match loader.to_ascii_lowercase().as_str() {
        "forge" => Some(1),
        "fabric" => Some(4),
        "quilt" => Some(5),
        "neoforge" => Some(6),
        _ => None,
    }
}

fn curseforge_kind(class_id: u64) -> String {
    match class_id {
        4471 => String::from("Modpack"),
        12 => String::from("Textura"),
        6552 => String::from("Shader"),
        5 => String::from("Plugin"),
        _ => String::from("Mod"),
    }
}

fn curseforge_file_download(file: &Value) -> Option<(String, String, Option<String>)> {
    let url = file.get("downloadUrl").and_then(Value::as_str)?.to_owned();
    let filename = file
        .get("fileName")
        .or_else(|| file.get("displayName"))
        .and_then(Value::as_str)
        .unwrap_or("conteudo.jar")
        .to_owned();
    let md5 = file
        .get("hashes")
        .and_then(Value::as_array)
        .and_then(|hashes| {
            hashes
                .iter()
                .find(|hash| hash.get("algo").and_then(Value::as_u64) == Some(2))
        })
        .and_then(|hash| hash.get("value"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    Some((url, filename, md5))
}

async fn curseforge_files(
    client: &reqwest::Client,
    project_id: &str,
    game_version: Option<&str>,
    loader: Option<&str>,
    page_size: u32,
) -> Result<Vec<Value>, String> {
    let mut request = client
        .get(format!(
            "https://api.curseforge.com/v1/mods/{project_id}/files"
        ))
        .query(&[("pageSize", page_size.min(50).to_string())]);
    if let Some(version) = game_version.filter(|value| !value.is_empty()) {
        request = request.query(&[("gameVersion", version)]);
    }
    if let Some(loader_type) = loader.and_then(curseforge_loader_type) {
        request = request.query(&[("modLoaderType", loader_type.to_string())]);
    }
    let response: Value = request
        .send()
        .await
        .map_err(|error| error.to_string())?
        .error_for_status()
        .map_err(|error| error.to_string())?
        .json()
        .await
        .map_err(|error| error.to_string())?;
    Ok(response
        .get("data")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default())
}

#[tauri::command]
async fn search_curseforge(
    query: String,
    project_type: String,
    game_version: String,
    loader: String,
    index: u32,
    page_size: u32,
) -> Result<CurseForgeSearchResult, String> {
    let client = curseforge_client()?;
    let mut request = client
        .get("https://api.curseforge.com/v1/mods/search")
        .query(&[
            ("gameId", String::from("432")),
            ("searchFilter", query.trim().to_owned()),
            ("index", index.to_string()),
            ("pageSize", page_size.clamp(1, 50).to_string()),
            ("sortField", String::from("6")),
            ("sortOrder", String::from("desc")),
        ]);
    if let Some(class_id) = curseforge_class_id(&project_type) {
        request = request.query(&[("classId", class_id.to_string())]);
    }
    if !game_version.trim().is_empty() {
        request = request.query(&[("gameVersion", game_version.trim())]);
    }
    if let Some(loader_type) = curseforge_loader_type(&loader) {
        request = request.query(&[("modLoaderType", loader_type.to_string())]);
    }
    let response: Value = request
        .send()
        .await
        .map_err(|error| error.to_string())?
        .error_for_status()
        .map_err(|error| error.to_string())?
        .json()
        .await
        .map_err(|error| error.to_string())?;
    let projects = response
        .get("data")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|project| {
            let mut versions = Vec::new();
            for version in project
                .get("latestFilesIndexes")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|item| item.get("gameVersion").and_then(Value::as_str))
            {
                if !versions.iter().any(|known| known == version) {
                    versions.push(version.to_owned());
                }
            }
            let class_id = project.get("classId").and_then(Value::as_u64).unwrap_or(6);
            CurseForgeProject {
                id: project
                    .get("id")
                    .and_then(Value::as_u64)
                    .unwrap_or_default()
                    .to_string(),
                name: project
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("Projeto CurseForge")
                    .to_owned(),
                author: project
                    .get("authors")
                    .and_then(Value::as_array)
                    .and_then(|authors| authors.first())
                    .and_then(|author| author.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or("CurseForge")
                    .to_owned(),
                kind: curseforge_kind(class_id),
                description: project
                    .get("summary")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                versions,
                downloads: project
                    .get("downloadCount")
                    .and_then(Value::as_f64)
                    .unwrap_or_default() as u64,
                icon_url: project
                    .get("logo")
                    .and_then(|logo| logo.get("thumbnailUrl").or_else(|| logo.get("url")))
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                page_url: project
                    .get("links")
                    .and_then(|links| links.get("websiteUrl"))
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            }
        })
        .collect();
    let total = response
        .get("pagination")
        .and_then(|pagination| pagination.get("totalCount"))
        .and_then(Value::as_u64)
        .unwrap_or_default();
    Ok(CurseForgeSearchResult { projects, total })
}

#[tauri::command]
async fn get_curseforge_project_versions(project_id: String) -> Result<Vec<String>, String> {
    let client = curseforge_client()?;
    let files = curseforge_files(&client, &project_id, None, None, 50).await?;
    let mut versions = Vec::new();
    for version in files
        .iter()
        .filter_map(|file| file.get("gameVersions").and_then(Value::as_array))
        .flatten()
        .filter_map(Value::as_str)
        .filter(|version| {
            version
                .chars()
                .next()
                .is_some_and(|value| value.is_ascii_digit())
        })
    {
        if !versions.iter().any(|known| known == version) {
            versions.push(version.to_owned());
        }
    }
    Ok(versions)
}

#[tauri::command]
async fn get_curseforge_install_targets(
    project_id: String,
    project_type: String,
    game_version: String,
) -> Result<Vec<CurseForgeInstallTarget>, String> {
    let client = curseforge_client()?;
    let folder = match project_type.as_str() {
        "mod" => "mods",
        "resourcepack" => "resourcepacks",
        "shader" => "shaderpacks",
        "plugin" => "plugins",
        _ => return Err(String::from("Este tipo usa o instalador de modpacks.")),
    };
    let mut targets = Vec::new();
    for instance in list_installed_instances()
        .into_iter()
        .filter(|instance| instance.mc_version == game_version)
    {
        let files = curseforge_files(
            &client,
            &project_id,
            Some(&game_version),
            (project_type == "mod").then_some(instance.loader.as_str()),
            20,
        )
        .await?;
        let Some((download_url, filename, md5)) = files.iter().find_map(curseforge_file_download)
        else {
            continue;
        };
        targets.push(CurseForgeInstallTarget {
            instance_name: instance.name,
            game_version: instance.mc_version,
            loader: instance.loader,
            destination_dir: PathBuf::from(instance.profile_dir)
                .join(folder)
                .to_string_lossy()
                .into_owned(),
            download_url,
            filename,
            md5,
        });
    }
    Ok(targets)
}

#[tauri::command]
async fn install_curseforge_target(
    app: tauri::AppHandle,
    target: CurseForgeInstallTarget,
) -> Result<String, String> {
    let operation = "install-content";
    validate_curseforge_download_url(&target.download_url)?;
    let settings = read_settings();
    let destination_dir = PathBuf::from(&target.destination_dir);
    let game_dir = PathBuf::from(&settings.game_directory);
    let servers = storage_root().join("servers");
    if !destination_dir.starts_with(&game_dir) && !destination_dir.starts_with(&servers) {
        return Err(String::from("Destino fora das pastas protegidas do VEX."));
    }
    let filename = Path::new(&target.filename)
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| String::from("Nome de arquivo inválido."))?;
    fs::create_dir_all(&destination_dir).map_err(|error| error.to_string())?;
    emit_progress(&app, operation, "Baixando conteúdo do CurseForge", 5, false);
    let client = curseforge_client()?;
    let bytes = download_bytes_with_progress(
        &app,
        operation,
        "Baixando conteúdo do CurseForge",
        &client,
        &target.download_url,
        8,
        88,
    )
    .await?;
    if let Some(expected) = target.md5.as_deref() {
        let actual = format!("{:x}", md5::compute(&bytes));
        if !actual.eq_ignore_ascii_case(expected) {
            return Err(String::from(
                "O arquivo não passou na verificação MD5 do CurseForge.",
            ));
        }
    }
    let destination = destination_dir.join(filename);
    fs::write(&destination, bytes).map_err(|error| error.to_string())?;
    emit_progress(&app, operation, "Conteúdo instalado", 100, true);
    Ok(destination.to_string_lossy().into_owned())
}

#[tauri::command]
fn read_latest_log() -> String {
    fs::read_to_string(storage_root().join("logs").join("latest.log")).unwrap_or_default()
}

#[tauri::command]
async fn get_modrinth_install_targets(
    project_id: String,
    project_type: String,
    game_version: String,
) -> Result<Vec<ModrinthInstallTarget>, String> {
    let client = reqwest::Client::builder()
        .user_agent(VEX_USER_AGENT)
        .build()
        .map_err(|error| error.to_string())?;
    let versions: Value = client
        .get(format!(
            "https://api.modrinth.com/v2/project/{project_id}/version"
        ))
        .send()
        .await
        .map_err(|error| error.to_string())?
        .error_for_status()
        .map_err(|error| error.to_string())?
        .json()
        .await
        .map_err(|error| error.to_string())?;
    let versions = versions
        .as_array()
        .ok_or_else(|| String::from("Resposta inválida do Modrinth."))?;
    let folder = match project_type.as_str() {
        "mod" => "mods",
        "resourcepack" => "resourcepacks",
        "shader" => "shaderpacks",
        "plugin" => "plugins",
        _ => {
            return Err(String::from(
                "Este tipo de projeto ainda não possui instalação automática.",
            ))
        }
    };

    let mut targets = Vec::new();
    for instance in list_installed_instances() {
        if instance.mc_version != game_version {
            continue;
        }
        let compatible = versions.iter().find(|version| {
            let supports_game = version
                .get("game_versions")
                .and_then(Value::as_array)
                .is_some_and(|items| {
                    items
                        .iter()
                        .any(|item| item.as_str() == Some(&instance.mc_version))
                });
            let supports_loader = match project_type.as_str() {
                "mod" => version
                    .get("loaders")
                    .and_then(Value::as_array)
                    .is_some_and(|items| {
                        items.iter().any(|item| {
                            item.as_str()
                                .is_some_and(|loader| loader.eq_ignore_ascii_case(&instance.loader))
                        })
                    }),
                "plugin" => false,
                _ => true,
            };
            supports_game && supports_loader
        });
        let Some(compatible) = compatible else {
            continue;
        };
        let file = compatible
            .get("files")
            .and_then(Value::as_array)
            .and_then(|files| {
                files
                    .iter()
                    .find(|file| file.get("primary").and_then(Value::as_bool) == Some(true))
                    .or_else(|| files.first())
            });
        let Some(file) = file else { continue };
        let Some(download_url) = file.get("url").and_then(Value::as_str) else {
            continue;
        };
        let filename = file
            .get("filename")
            .and_then(Value::as_str)
            .unwrap_or("content.jar");
        targets.push(ModrinthInstallTarget {
            instance_name: instance.name,
            game_version: instance.mc_version,
            loader: instance.loader,
            destination_dir: PathBuf::from(instance.profile_dir)
                .join(folder)
                .to_string_lossy()
                .into_owned(),
            download_url: download_url.to_owned(),
            filename: filename.to_owned(),
            sha512: file
                .get("hashes")
                .and_then(|hashes| hashes.get("sha512"))
                .and_then(Value::as_str)
                .map(str::to_owned),
        });
    }
    let server = read_server_profile();
    let server_matches = server.version == game_version
        && ((project_type == "plugin" && server.software == "paper")
            || (project_type == "mod" && server.software == "fabric"));
    if server_matches {
        let compatible = versions.iter().find(|version| {
            let supports_game = version
                .get("game_versions")
                .and_then(Value::as_array)
                .is_some_and(|items| {
                    items
                        .iter()
                        .any(|item| item.as_str() == Some(&server.version))
                });
            let supports_loader = version
                .get("loaders")
                .and_then(Value::as_array)
                .is_some_and(|items| {
                    items.iter().any(|item| {
                        item.as_str().is_some_and(|loader| {
                            if server.software == "paper" {
                                matches!(
                                    loader.to_lowercase().as_str(),
                                    "paper" | "spigot" | "bukkit" | "purpur"
                                )
                            } else {
                                loader.eq_ignore_ascii_case("fabric")
                            }
                        })
                    })
                });
            supports_game && supports_loader
        });
        if let Some(file) = compatible
            .and_then(|version| version.get("files"))
            .and_then(Value::as_array)
            .and_then(|files| {
                files
                    .iter()
                    .find(|file| file.get("primary").and_then(Value::as_bool) == Some(true))
                    .or_else(|| files.first())
            })
        {
            if let Some(download_url) = file.get("url").and_then(Value::as_str) {
                targets.push(ModrinthInstallTarget {
                    instance_name: format!("Servidor: {}", server.name),
                    game_version: server.version,
                    loader: server.software,
                    destination_dir: PathBuf::from(server.directory)
                        .join(folder)
                        .to_string_lossy()
                        .into_owned(),
                    download_url: download_url.to_owned(),
                    filename: file
                        .get("filename")
                        .and_then(Value::as_str)
                        .unwrap_or("content.jar")
                        .to_owned(),
                    sha512: file
                        .get("hashes")
                        .and_then(|hashes| hashes.get("sha512"))
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                });
            }
        }
    }
    Ok(targets)
}

#[tauri::command]
async fn install_modrinth_target(
    app: tauri::AppHandle,
    target: ModrinthInstallTarget,
) -> Result<String, String> {
    let operation = "install-content";
    validate_modrinth_download_url(&target.download_url)?;
    emit_progress(
        &app,
        operation,
        format!("Preparando instalação em {}", target.instance_name),
        4,
        false,
    );
    let settings = read_settings();
    let destination_dir = PathBuf::from(&target.destination_dir);
    let game_dir = PathBuf::from(&settings.game_directory);
    let registered_server_destination = read_server_registry().servers.iter().any(|profile| {
        current_server_directory(profile).is_ok_and(|directory| {
            [directory.join("mods"), directory.join("plugins")]
                .iter()
                .any(|candidate| candidate == &destination_dir)
        })
    });
    if !destination_dir.starts_with(&game_dir) && !registered_server_destination {
        return Err(String::from(
            "O destino precisa ser a pasta de uma instância ou mods/plugins de um servidor registrado.",
        ));
    }
    let _server_operation = if registered_server_destination {
        let guard = begin_server_operation()?;
        ensure_server_stopped_for_profile_change()?;
        Some(guard)
    } else {
        None
    };
    fs::create_dir_all(&destination_dir).map_err(|error| error.to_string())?;
    let filename = Path::new(&target.filename)
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| String::from("Nome de arquivo inválido."))?;
    let client = reqwest::Client::builder()
        .user_agent(VEX_USER_AGENT)
        .build()
        .map_err(|error| error.to_string())?;
    let bytes = download_bytes_with_progress(
        &app,
        operation,
        "Baixando conteúdo",
        &client,
        &target.download_url,
        8,
        82,
    )
    .await?;
    emit_progress(&app, operation, "Verificando integridade", 90, false);
    if let Some(expected) = target.sha512.as_deref() {
        let actual = format!("{:x}", Sha512::digest(&bytes));
        if !actual.eq_ignore_ascii_case(expected) {
            return Err(String::from(
                "O download não passou na verificação de integridade SHA-512.",
            ));
        }
    }
    let destination = destination_dir.join(filename);
    fs::write(&destination, bytes).map_err(|error| error.to_string())?;
    emit_progress(&app, operation, "Conteúdo instalado", 100, true);
    Ok(destination.to_string_lossy().into_owned())
}

fn safe_directory_name(name: &str) -> String {
    name.chars()
        .map(|character| {
            if matches!(
                character,
                '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
            ) || character.is_control()
            {
                '_'
            } else {
                character
            }
        })
        .collect::<String>()
        .trim()
        .trim_end_matches('.')
        .to_owned()
}

async fn install_fabric_profile(
    client: &reqwest::Client,
    game_dir: &Path,
    minecraft: &str,
    loader: &str,
) -> Result<String, String> {
    let profile_id = format!("fabric-loader-{loader}-{minecraft}");
    let destination = game_dir
        .join("versions")
        .join(&profile_id)
        .join(format!("{profile_id}.json"));
    if !destination.is_file() {
        let url = format!(
            "https://meta.fabricmc.net/v2/versions/loader/{minecraft}/{loader}/profile/json"
        );
        download_to(client, &url, &destination).await?;
    }
    Ok(profile_id)
}

async fn install_quilt_profile(
    client: &reqwest::Client,
    game_dir: &Path,
    minecraft: &str,
) -> Result<String, String> {
    let loaders: Value = client
        .get(format!(
            "https://meta.quiltmc.org/v3/versions/loader/{minecraft}"
        ))
        .send()
        .await
        .map_err(|error| error.to_string())?
        .error_for_status()
        .map_err(|error| error.to_string())?
        .json()
        .await
        .map_err(|error| error.to_string())?;
    let loader = loaders
        .as_array()
        .and_then(|items| items.first())
        .and_then(|item| item.get("loader"))
        .and_then(|item| item.get("version"))
        .and_then(Value::as_str)
        .ok_or_else(|| format!("Quilt não está disponível para Minecraft {minecraft}."))?;
    let profile_id = format!("quilt-loader-{loader}-{minecraft}");
    let destination = game_dir
        .join("versions")
        .join(&profile_id)
        .join(format!("{profile_id}.json"));
    if !destination.is_file() {
        let url = format!(
            "https://meta.quiltmc.org/v3/versions/loader/{minecraft}/{loader}/profile/json"
        );
        download_to(client, &url, &destination).await?;
    }
    Ok(profile_id)
}

fn minecraft_version_parts(version: &str) -> (u32, u32, u32) {
    let mut parts = version
        .split('.')
        .map(|part| part.parse::<u32>().unwrap_or_default());
    (
        parts.next().unwrap_or_default(),
        parts.next().unwrap_or_default(),
        parts.next().unwrap_or_default(),
    )
}

fn required_java_for_minecraft(version: &str) -> u32 {
    let (_, minor, patch) = minecraft_version_parts(version);
    if minor > 20 || (minor == 20 && patch >= 5) {
        21
    } else if minor >= 18 {
        17
    } else if minor >= 17 {
        16
    } else {
        8
    }
}

fn metadata_versions(xml: &str) -> Vec<String> {
    xml.split("<version>")
        .skip(1)
        .filter_map(|part| part.split("</version>").next())
        .map(str::trim)
        .filter(|version| !version.is_empty())
        .map(str::to_owned)
        .collect()
}

fn loader_metadata_url(loader: &str) -> Option<&'static str> {
    match loader {
        "forge" => {
            Some("https://maven.minecraftforge.net/net/minecraftforge/forge/maven-metadata.xml")
        }
        "neoforge" => {
            Some("https://maven.neoforged.net/releases/net/neoforged/neoforge/maven-metadata.xml")
        }
        _ => None,
    }
}

fn official_loader_minecraft_version(loader: &str, loader_version: &str) -> Option<String> {
    if loader == "forge" {
        return loader_version
            .split_once('-')
            .map(|(minecraft, _)| minecraft.to_owned());
    }
    if loader != "neoforge" {
        return None;
    }
    let stable = loader_version.split('-').next()?;
    let numbers: Vec<u32> = stable
        .split('.')
        .filter_map(|part| part.parse::<u32>().ok())
        .collect();
    if numbers.len() < 2 {
        return None;
    }
    if numbers[0] == 47 {
        return Some(String::from("1.20.1"));
    }
    if numbers[0] >= 26 {
        return Some(format!(
            "{}.{}.{}",
            numbers[0],
            numbers[1],
            numbers.get(2).copied().unwrap_or_default()
        ));
    }
    if numbers[1] == 0 {
        Some(format!("1.{}", numbers[0]))
    } else {
        Some(format!("1.{}.{}", numbers[0], numbers[1]))
    }
}

async fn official_loader_versions(
    client: &reqwest::Client,
    loader: &str,
) -> Result<Vec<(String, String)>, String> {
    let metadata_url =
        loader_metadata_url(loader).ok_or_else(|| format!("Unknown loader: {loader}"))?;
    let xml = client
        .get(metadata_url)
        .send()
        .await
        .map_err(|error| error.to_string())?
        .error_for_status()
        .map_err(|error| error.to_string())?
        .text()
        .await
        .map_err(|error| error.to_string())?;
    Ok(metadata_versions(&xml)
        .into_iter()
        .rev()
        .filter_map(|loader_version| {
            official_loader_minecraft_version(loader, &loader_version)
                .map(|minecraft| (minecraft, loader_version))
        })
        .collect())
}

async fn resolve_official_loader_version(
    client: &reqwest::Client,
    minecraft: &str,
    loader: &str,
    requested: Option<&str>,
) -> Result<String, String> {
    if let Some(requested) = requested.filter(|value| !value.trim().is_empty()) {
        let clean = requested
            .trim()
            .strip_prefix("forge-")
            .or_else(|| requested.trim().strip_prefix("neoforge-"))
            .unwrap_or(requested.trim());
        return Ok(
            if loader == "forge" && !clean.starts_with(&format!("{minecraft}-")) {
                format!("{minecraft}-{clean}")
            } else {
                clean.to_owned()
            },
        );
    }
    official_loader_versions(client, loader)
        .await?
        .into_iter()
        .find_map(|(game_version, loader_version)| {
            (game_version == minecraft).then_some(loader_version)
        })
        .ok_or_else(|| format!("{loader} is not available for Minecraft {minecraft}."))
}

fn find_installed_loader_profile(game_dir: &Path, minecraft: &str, loader: &str) -> Option<String> {
    let versions = game_dir.join("versions");
    let mut candidates: Vec<(u64, String)> = fs::read_dir(versions)
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let id = entry.file_name().to_string_lossy().into_owned();
            let lower = id.to_ascii_lowercase();
            let json = entry.path().join(format!("{id}.json"));
            (json.is_file()
                && lower.contains(loader)
                && (lower.contains(minecraft) || loader == "neoforge"))
                .then(|| (modified_unix(&json), id))
        })
        .collect();
    candidates.sort_by_key(|candidate| std::cmp::Reverse(candidate.0));
    candidates.into_iter().next().map(|(_, id)| id)
}

fn read_installer_profile(installer: &Path) -> Result<Value, String> {
    let file = fs::File::open(installer).map_err(|error| error.to_string())?;
    let mut archive = zip::ZipArchive::new(file).map_err(|error| error.to_string())?;
    let mut entry = archive
        .by_name("install_profile.json")
        .map_err(|error| format!("Installer does not contain install_profile.json: {error}"))?;
    let mut raw = String::new();
    entry
        .read_to_string(&mut raw)
        .map_err(|error| error.to_string())?;
    serde_json::from_str(&raw).map_err(|error| error.to_string())
}

fn ensure_launcher_profile_stub(game_dir: &Path) -> Result<Option<PathBuf>, String> {
    let legacy = game_dir.join("launcher_profiles.json");
    let microsoft_store = game_dir.join("launcher_profiles_microsoft_store.json");
    if legacy.is_file() || microsoft_store.is_file() {
        return Ok(None);
    }
    fs::write(&legacy, "{\n  \"profiles\": {},\n  \"settings\": {}\n}")
        .map_err(|error| error.to_string())?;
    Ok(Some(legacy))
}

#[allow(clippy::too_many_arguments)]
async fn install_vex_loader_profile(
    app: &tauri::AppHandle,
    game_dir: &Path,
    minecraft: &str,
    loader: &str,
    requested_loader_version: Option<&str>,
    operation: &str,
    start: u8,
    end: u8,
) -> Result<String, String> {
    let client = reqwest::Client::builder()
        .user_agent(VEX_USER_AGENT)
        .build()
        .map_err(|error| error.to_string())?;
    emit_progress(
        app,
        operation,
        format!("Checking official {loader} versions"),
        start,
        false,
    );
    let loader_version =
        resolve_official_loader_version(&client, minecraft, loader, requested_loader_version)
            .await?;
    let (url, filename) = if loader == "forge" {
        (
            format!(
                "https://maven.minecraftforge.net/net/minecraftforge/forge/{loader_version}/forge-{loader_version}-installer.jar"
            ),
            format!("forge-{loader_version}-installer.jar"),
        )
    } else {
        (
            format!(
                "https://maven.neoforged.net/releases/net/neoforged/neoforge/{loader_version}/neoforge-{loader_version}-installer.jar"
            ),
            format!("neoforge-{loader_version}-installer.jar"),
        )
    };
    let installer_dir = storage_root().join("cache").join("loaders");
    fs::create_dir_all(&installer_dir).map_err(|error| error.to_string())?;
    let installer = installer_dir.join(filename);
    if !installer.is_file() {
        let bytes = download_bytes_with_progress(
            app,
            operation,
            &format!("Downloading official {loader} installer"),
            &client,
            &url,
            start.saturating_add(24),
            end.saturating_sub(25),
        )
        .await?;
        fs::write(&installer, bytes).map_err(|error| error.to_string())?;
    }
    let install_profile = read_installer_profile(&installer)?;
    let installer_minecraft = install_profile
        .get("minecraft")
        .and_then(Value::as_str)
        .unwrap_or(minecraft);
    if installer_minecraft != minecraft {
        return Err(format!(
            "{loader} {loader_version} targets Minecraft {installer_minecraft}, not {minecraft}."
        ));
    }
    let expected_profile = install_profile
        .get("version")
        .and_then(Value::as_str)
        .map(str::to_owned);
    if let Some(profile_id) = expected_profile.as_deref() {
        let json = game_dir
            .join("versions")
            .join(profile_id)
            .join(format!("{profile_id}.json"));
        if json.is_file() {
            return Ok(profile_id.to_owned());
        }
    }
    fs::create_dir_all(game_dir).map_err(|error| error.to_string())?;
    let vanilla_profile = get_version_json(&client, game_dir, minecraft).await?;
    let required_java = vanilla_profile
        .get("javaVersion")
        .and_then(|java| java.get("majorVersion"))
        .and_then(Value::as_u64)
        .map(|version| version as u32)
        .unwrap_or_else(|| required_java_for_minecraft(minecraft));
    let runtime = ensure_java_runtime(
        app,
        required_java,
        operation,
        start.saturating_add(3),
        start.saturating_add(22),
    )
    .await?;
    let launcher_profile_stub = ensure_launcher_profile_stub(game_dir)?;
    emit_progress(
        app,
        operation,
        format!("Installing {loader} with the VEX native flow"),
        end.saturating_sub(20),
        false,
    );
    let output_result = hidden_command(&runtime.path)
        .arg("-jar")
        .arg(&installer)
        .arg("--installClient")
        .arg(game_dir)
        .current_dir(game_dir)
        .output();
    if let Some(stub) = launcher_profile_stub {
        let _ = fs::remove_file(stub);
    }
    let output = output_result
        .map_err(|error| format!("Could not run the official {loader} installer: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "The official {loader} installer failed: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let profile_id = expected_profile
        .or_else(|| find_installed_loader_profile(game_dir, minecraft, loader))
        .ok_or_else(|| {
            format!(
                "The {loader} installer finished, but VEX could not find the generated profile."
            )
        })?;
    let profile_json = game_dir
        .join("versions")
        .join(&profile_id)
        .join(format!("{profile_id}.json"));
    if !profile_json.is_file() {
        return Err(format!(
            "The {loader} installer reported success, but profile {profile_id} is missing."
        ));
    }
    emit_progress(app, operation, format!("{loader} installed"), end, false);
    Ok(profile_id)
}

#[tauri::command]
async fn list_loader_game_versions(loader: String) -> Result<Vec<LoaderGameVersion>, String> {
    let clean_loader = loader.trim().to_ascii_lowercase();
    let client = reqwest::Client::builder()
        .user_agent(VEX_USER_AGENT)
        .build()
        .map_err(|error| error.to_string())?;
    let available = official_loader_versions(&client, &clean_loader).await?;
    let mut seen = HashSet::new();
    let mut versions = Vec::new();
    for (game_version, loader_version) in available {
        if !seen.insert(game_version.to_owned()) {
            continue;
        }
        versions.push(LoaderGameVersion {
            game_version,
            loader_version,
            recommended: false,
        });
    }
    Ok(versions)
}

#[tauri::command]
async fn check_modpack_update(profile_dir: String) -> Result<ModpackUpdateStatus, String> {
    let instance_dir = validated_instance_dir(&profile_dir)?;
    let metadata: Value = serde_json::from_str(
        &fs::read_to_string(instance_dir.join("instance.json"))
            .map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    let source = metadata
        .get("Source")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let project_name = metadata
        .get("Name")
        .and_then(Value::as_str)
        .unwrap_or("Modpack")
        .to_owned();
    let author = metadata
        .get("Author")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let game_version = metadata
        .get("McVersion")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let project_id = metadata
        .get("ProjectId")
        .or_else(|| metadata.get("Id"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim_start_matches("modrinth-")
        .to_owned();
    let current_version = metadata
        .get("ProjectVersionNumber")
        .and_then(Value::as_str)
        .unwrap_or("Desconhecida")
        .to_owned();
    if !source.eq_ignore_ascii_case("modrinth") || project_id.is_empty() {
        return Ok(ModpackUpdateStatus {
            supported: false,
            update_available: false,
            project_id,
            project_name,
            author,
            game_version,
            current_version,
            latest_version: String::new(),
            latest_version_id: String::new(),
            message: String::from(
                "Atualizacao automatica esta disponivel para modpacks instalados pelo Modrinth.",
            ),
        });
    }
    let client = reqwest::Client::builder()
        .user_agent(VEX_USER_AGENT)
        .build()
        .map_err(|error| error.to_string())?;
    let versions: Value = client
        .get(format!(
            "https://api.modrinth.com/v2/project/{project_id}/version"
        ))
        .send()
        .await
        .map_err(|error| error.to_string())?
        .error_for_status()
        .map_err(|error| error.to_string())?
        .json()
        .await
        .map_err(|error| error.to_string())?;
    let latest = versions
        .as_array()
        .and_then(|items| {
            items.iter().find(|version| {
                let game_matches = version
                    .get("game_versions")
                    .and_then(Value::as_array)
                    .is_some_and(|versions| {
                        versions
                            .iter()
                            .any(|version| version.as_str() == Some(&game_version))
                    });
                let has_pack =
                    version
                        .get("files")
                        .and_then(Value::as_array)
                        .is_some_and(|files| {
                            files.iter().any(|file| {
                                file.get("filename")
                                    .and_then(Value::as_str)
                                    .is_some_and(|name| name.ends_with(".mrpack"))
                            })
                        });
                game_matches && has_pack
            })
        })
        .ok_or_else(|| format!("Nenhuma versao do modpack suporta Minecraft {game_version}."))?;
    let latest_version_id = latest
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let latest_version = latest
        .get("version_number")
        .and_then(Value::as_str)
        .unwrap_or("Mais recente")
        .to_owned();
    let current_version_id = metadata
        .get("ProjectVersionId")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let update_available = current_version_id.is_empty() || current_version_id != latest_version_id;
    Ok(ModpackUpdateStatus {
        supported: true,
        update_available,
        project_id,
        project_name,
        author,
        game_version,
        current_version,
        latest_version: latest_version.clone(),
        latest_version_id,
        message: if update_available {
            format!("A versao {latest_version} esta disponivel.")
        } else {
            String::from("O modpack ja esta atualizado.")
        },
    })
}

#[tauri::command]
async fn install_modrinth_modpack(
    app: tauri::AppHandle,
    project_id: String,
    project_name: String,
    author: String,
    game_version: String,
    target_profile_dir: Option<String>,
) -> Result<InstalledInstance, String> {
    let operation = "install-modpack";
    emit_progress(&app, operation, "Consultando versões do modpack", 3, false);
    let settings = read_settings();
    let game_dir = PathBuf::from(&settings.game_directory);
    let client = reqwest::Client::builder()
        .user_agent(VEX_USER_AGENT)
        .build()
        .map_err(|error| error.to_string())?;
    let versions: Value = client
        .get(format!(
            "https://api.modrinth.com/v2/project/{project_id}/version"
        ))
        .send()
        .await
        .map_err(|error| error.to_string())?
        .error_for_status()
        .map_err(|error| error.to_string())?
        .json()
        .await
        .map_err(|error| error.to_string())?;
    let versions = versions
        .as_array()
        .ok_or_else(|| String::from("Resposta inválida do Modrinth."))?;
    let selected = versions
        .iter()
        .find(|version| {
            let game_matches = game_version.is_empty()
                || version
                    .get("game_versions")
                    .and_then(Value::as_array)
                    .is_some_and(|items| {
                        items
                            .iter()
                            .any(|item| item.as_str() == Some(&game_version))
                    });
            let has_pack = version
                .get("files")
                .and_then(Value::as_array)
                .is_some_and(|files| {
                    files.iter().any(|file| {
                        file.get("filename")
                            .and_then(Value::as_str)
                            .is_some_and(|name| name.ends_with(".mrpack"))
                    })
                });
            game_matches && has_pack
        })
        .ok_or_else(|| {
            format!("Nenhum arquivo .mrpack disponível para Minecraft {game_version}.")
        })?;
    let selected_version_id = selected
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let selected_version_number = selected
        .get("version_number")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let pack_file = selected
        .get("files")
        .and_then(Value::as_array)
        .and_then(|files| {
            files.iter().find(|file| {
                file.get("filename")
                    .and_then(Value::as_str)
                    .is_some_and(|name| name.ends_with(".mrpack"))
            })
        })
        .ok_or_else(|| String::from("Arquivo .mrpack não encontrado."))?;
    let pack_url = pack_file
        .get("url")
        .and_then(Value::as_str)
        .ok_or_else(|| String::from("Arquivo .mrpack sem URL."))?;
    validate_modrinth_download_url(pack_url)?;
    let pack_bytes = download_bytes_with_progress(
        &app,
        operation,
        "Baixando pacote principal",
        &client,
        pack_url,
        7,
        24,
    )
    .await?;
    emit_progress(&app, operation, "Verificando pacote", 27, false);
    if let Some(expected) = pack_file
        .get("hashes")
        .and_then(|hashes| hashes.get("sha512"))
        .and_then(Value::as_str)
    {
        let actual = format!("{:x}", Sha512::digest(&pack_bytes));
        if !actual.eq_ignore_ascii_case(expected) {
            return Err(String::from("O .mrpack não passou na verificação SHA-512."));
        }
    }

    let instance_name = safe_directory_name(&project_name);
    if instance_name.is_empty() {
        return Err(String::from("Nome de modpack inválido."));
    }
    let instance_dir = if let Some(profile_dir) = target_profile_dir.as_deref() {
        validated_instance_dir(profile_dir)?
    } else {
        game_dir.join("modpacks").join(&instance_name)
    };
    let previous_metadata = fs::read_to_string(instance_dir.join("instance.json"))
        .ok()
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok());
    let backup_path = if target_profile_dir.is_some() {
        emit_progress(
            &app,
            operation,
            "Criando backup antes da atualizacao",
            29,
            false,
        );
        Some(backup_instance_directory(&instance_dir)?)
    } else {
        None
    };
    fs::create_dir_all(&instance_dir).map_err(|error| error.to_string())?;
    let mut archive =
        zip::ZipArchive::new(Cursor::new(pack_bytes)).map_err(|error| error.to_string())?;
    emit_progress(&app, operation, "Extraindo configurações", 31, false);
    let mut index_text = String::new();
    archive
        .by_name("modrinth.index.json")
        .map_err(|_| String::from("O pacote não contém modrinth.index.json."))?
        .read_to_string(&mut index_text)
        .map_err(|error| error.to_string())?;
    let index: Value = serde_json::from_str(&index_text).map_err(|error| error.to_string())?;
    let dependencies = index
        .get("dependencies")
        .and_then(Value::as_object)
        .ok_or_else(|| String::from("Modpack sem dependências declaradas."))?;
    let minecraft = dependencies
        .get("minecraft")
        .and_then(Value::as_str)
        .ok_or_else(|| String::from("Modpack sem versão Minecraft."))?;
    let fabric_loader = dependencies.get("fabric-loader").and_then(Value::as_str);
    let quilt_loader = dependencies.get("quilt-loader").and_then(Value::as_str);
    let forge_loader = dependencies.get("forge").and_then(Value::as_str);
    let neoforge_loader = dependencies.get("neoforge").and_then(Value::as_str);
    let loader = if fabric_loader.is_some() {
        "fabric"
    } else if quilt_loader.is_some() {
        "quilt"
    } else if neoforge_loader.is_some() {
        "neoforge"
    } else if forge_loader.is_some() {
        "forge"
    } else {
        "vanilla"
    };

    let mut managed_files: Vec<String> = Vec::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|error| error.to_string())?;
        if entry.is_dir() {
            continue;
        }
        let Some(enclosed) = entry.enclosed_name() else {
            continue;
        };
        let relative = enclosed
            .strip_prefix("overrides")
            .or_else(|_| enclosed.strip_prefix("client-overrides"));
        let Ok(relative) = relative else { continue };
        managed_files.push(relative.to_string_lossy().replace('\\', "/"));
        let destination = instance_dir.join(relative);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let mut output = fs::File::create(destination).map_err(|error| error.to_string())?;
        std::io::copy(&mut entry, &mut output).map_err(|error| error.to_string())?;
    }

    let files = index
        .get("files")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let file_count = files.len().max(1);
    for (file_index, file) in files.into_iter().enumerate() {
        let Some(relative) = file.get("path").and_then(Value::as_str) else {
            continue;
        };
        let relative_path = PathBuf::from(relative.replace('/', std::path::MAIN_SEPARATOR_STR));
        if relative_path.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir
                    | std::path::Component::RootDir
                    | std::path::Component::Prefix(_)
            )
        }) {
            continue;
        }
        managed_files.push(relative.replace('\\', "/"));
        let destination = instance_dir.join(relative_path);
        let downloads = file
            .get("downloads")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for url in downloads.iter().filter_map(Value::as_str) {
            if download_to(&client, url, &destination).await.is_ok() {
                break;
            }
        }
        let percent = 35 + (((file_index + 1) as f64 / file_count as f64) * 48.0) as u8;
        emit_progress(
            &app,
            operation,
            format!(
                "Baixando arquivos do modpack ({}/{file_count})",
                file_index + 1
            ),
            percent,
            false,
        );
    }

    emit_progress(&app, operation, "Preparando loader e versão", 87, false);
    let version_id = if let Some(fabric) = fabric_loader {
        install_fabric_profile(&client, &game_dir, minecraft, fabric).await?
    } else if quilt_loader.is_some() {
        install_quilt_profile(&client, &game_dir, minecraft).await?
    } else if let Some(forge) = forge_loader {
        install_vex_loader_profile(
            &app,
            &game_dir,
            minecraft,
            "forge",
            Some(forge),
            operation,
            86,
            94,
        )
        .await?
    } else if let Some(neoforge) = neoforge_loader {
        install_vex_loader_profile(
            &app,
            &game_dir,
            minecraft,
            "neoforge",
            Some(neoforge),
            operation,
            86,
            94,
        )
        .await?
    } else {
        minecraft.to_owned()
    };
    let project: Value = client
        .get(format!("https://api.modrinth.com/v2/project/{project_id}"))
        .send()
        .await
        .map_err(|error| error.to_string())?
        .json()
        .await
        .map_err(|error| error.to_string())?;
    let mut icon_path = None;
    if let Some(icon_url) = project.get("icon_url").and_then(Value::as_str) {
        emit_progress(&app, operation, "Baixando identidade do modpack", 94, false);
        let destination = instance_dir.join("icon.png");
        if download_to(&client, icon_url, &destination).await.is_ok() {
            icon_path = Some(destination.to_string_lossy().into_owned());
        }
    }
    let metadata = serde_json::json!({
        "Id": project_id,
        "Name": project_name,
        "Author": author,
        "Source": "Modrinth",
        "ProjectId": project_id,
        "ProjectVersionId": selected_version_id,
        "ProjectVersionNumber": selected_version_number,
        "IconPath": icon_path.clone().unwrap_or_default(),
        "McVersion": minecraft,
        "Loader": loader,
        "LoaderVersion": fabric_loader.or(quilt_loader).or(forge_loader).or(neoforge_loader).unwrap_or_default(),
        "VersionId": version_id,
        "InstallDate": "installed-by-vex-launcher",
        "LastPlayedUnix": previous_metadata.as_ref().and_then(|value| value.get("LastPlayedUnix")).and_then(Value::as_u64).unwrap_or_default(),
        "ManagedFiles": managed_files,
        "LastBackupPath": backup_path.as_ref().map(|path| path.to_string_lossy().into_owned()).unwrap_or_default()
    });
    fs::write(
        instance_dir.join("instance.json"),
        serde_json::to_string_pretty(&metadata).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    emit_progress(&app, operation, "Modpack instalado", 100, true);
    Ok(InstalledInstance {
        id: metadata["Id"].as_str().unwrap_or_default().to_owned(),
        name: metadata["Name"].as_str().unwrap_or_default().to_owned(),
        loader: loader.to_owned(),
        mc_version: minecraft.to_owned(),
        version_id: metadata["VersionId"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
        profile_dir: instance_dir.to_string_lossy().into_owned(),
        icon_path,
        kind: String::from("modpack"),
        size_mb: (directory_size(&instance_dir) as f64 / 1_048_576.0 * 10.0).round() / 10.0,
        modified_unix: modified_unix(&instance_dir),
        last_played_unix: 0,
    })
}

#[tauri::command]
async fn install_curseforge_modpack(
    app: tauri::AppHandle,
    project_id: String,
    project_name: String,
    author: String,
    game_version: String,
) -> Result<InstalledInstance, String> {
    let operation = "install-modpack";
    emit_progress(
        &app,
        operation,
        "Consultando arquivos do CurseForge",
        3,
        false,
    );
    let settings = read_settings();
    let game_dir = PathBuf::from(&settings.game_directory);
    let client = curseforge_client()?;
    let files = curseforge_files(&client, &project_id, Some(&game_version), None, 50).await?;
    let (pack_url, _, expected_md5) = files
        .iter()
        .find_map(curseforge_file_download)
        .ok_or_else(|| {
            String::from(
                "O autor não permitiu download externo para esta versão. Abra a página do projeto no CurseForge.",
            )
        })?;
    validate_curseforge_download_url(&pack_url)?;
    let pack_bytes = download_bytes_with_progress(
        &app,
        operation,
        "Baixando modpack do CurseForge",
        &client,
        &pack_url,
        7,
        24,
    )
    .await?;
    if let Some(expected) = expected_md5.as_deref() {
        let actual = format!("{:x}", md5::compute(&pack_bytes));
        if !actual.eq_ignore_ascii_case(expected) {
            return Err(String::from(
                "O modpack não passou na verificação MD5 do CurseForge.",
            ));
        }
    }
    let instance_name = safe_directory_name(&project_name);
    let instance_dir = game_dir.join("modpacks").join(&instance_name);
    fs::create_dir_all(&instance_dir).map_err(|error| error.to_string())?;
    let mut archive =
        zip::ZipArchive::new(Cursor::new(pack_bytes)).map_err(|error| error.to_string())?;
    let mut manifest_text = String::new();
    archive
        .by_name("manifest.json")
        .map_err(|_| String::from("O modpack não contém manifest.json."))?
        .read_to_string(&mut manifest_text)
        .map_err(|error| error.to_string())?;
    let manifest: Value =
        serde_json::from_str(&manifest_text).map_err(|error| error.to_string())?;
    let minecraft = manifest
        .get("minecraft")
        .and_then(|minecraft| minecraft.get("version"))
        .and_then(Value::as_str)
        .ok_or_else(|| String::from("Manifesto CurseForge sem versão do Minecraft."))?;
    let loader_id = manifest
        .get("minecraft")
        .and_then(|minecraft| minecraft.get("modLoaders"))
        .and_then(Value::as_array)
        .and_then(|loaders| {
            loaders
                .iter()
                .find(|loader| loader.get("primary").and_then(Value::as_bool) == Some(true))
                .or_else(|| loaders.first())
        })
        .and_then(|loader| loader.get("id"))
        .and_then(Value::as_str)
        .unwrap_or("vanilla");
    let (loader, loader_version) = loader_id
        .split_once('-')
        .map(|(loader, version)| (loader.to_ascii_lowercase(), version))
        .unwrap_or_else(|| (String::from("vanilla"), ""));
    emit_progress(
        &app,
        operation,
        "Extraindo configurações do modpack",
        28,
        false,
    );
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|error| error.to_string())?;
        if entry.is_dir() {
            continue;
        }
        let Some(enclosed) = entry.enclosed_name() else {
            continue;
        };
        let relative = enclosed
            .strip_prefix("overrides")
            .or_else(|_| enclosed.strip_prefix("client-overrides"));
        let Ok(relative) = relative else { continue };
        let destination = instance_dir.join(relative);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let mut output = fs::File::create(destination).map_err(|error| error.to_string())?;
        std::io::copy(&mut entry, &mut output).map_err(|error| error.to_string())?;
    }
    let manifest_files = manifest
        .get("files")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let total_files = manifest_files.len().max(1);
    fs::create_dir_all(instance_dir.join("mods")).map_err(|error| error.to_string())?;
    for (index, file_ref) in manifest_files.into_iter().enumerate() {
        let project = file_ref
            .get("projectID")
            .and_then(Value::as_u64)
            .ok_or_else(|| String::from("Referência de projeto inválida no modpack."))?;
        let file = file_ref
            .get("fileID")
            .and_then(Value::as_u64)
            .ok_or_else(|| String::from("Referência de arquivo inválida no modpack."))?;
        let response: Value = client
            .get(format!(
                "https://api.curseforge.com/v1/mods/{project}/files/{file}"
            ))
            .send()
            .await
            .map_err(|error| error.to_string())?
            .error_for_status()
            .map_err(|error| error.to_string())?
            .json()
            .await
            .map_err(|error| error.to_string())?;
        let file_data = response
            .get("data")
            .ok_or_else(|| String::from("Arquivo ausente na resposta do CurseForge."))?;
        let (url, filename, expected) = curseforge_file_download(file_data).ok_or_else(|| {
            format!(
                "O autor do projeto {project} bloqueou downloads externos. Instale este arquivo pela página do CurseForge."
            )
        })?;
        validate_curseforge_download_url(&url)?;
        let bytes = client
            .get(url)
            .send()
            .await
            .map_err(|error| error.to_string())?
            .error_for_status()
            .map_err(|error| error.to_string())?
            .bytes()
            .await
            .map_err(|error| error.to_string())?;
        if let Some(expected) = expected {
            let actual = format!("{:x}", md5::compute(&bytes));
            if !actual.eq_ignore_ascii_case(&expected) {
                return Err(format!("O arquivo {filename} falhou na verificação MD5."));
            }
        }
        fs::write(instance_dir.join("mods").join(filename), bytes)
            .map_err(|error| error.to_string())?;
        emit_progress(
            &app,
            operation,
            format!("Baixando arquivos do modpack ({}/{total_files})", index + 1),
            32 + (((index + 1) as f64 / total_files as f64) * 48.0) as u8,
            false,
        );
    }
    let version_id = match loader.as_str() {
        "fabric" => install_fabric_profile(&client, &game_dir, minecraft, loader_version).await?,
        "quilt" => install_quilt_profile(&client, &game_dir, minecraft).await?,
        "forge" | "neoforge" => {
            install_vex_loader_profile(
                &app,
                &game_dir,
                minecraft,
                &loader,
                Some(loader_version),
                operation,
                82,
                95,
            )
            .await?
        }
        _ => minecraft.to_owned(),
    };
    let project_response: Value = client
        .get(format!("https://api.curseforge.com/v1/mods/{project_id}"))
        .send()
        .await
        .map_err(|error| error.to_string())?
        .error_for_status()
        .map_err(|error| error.to_string())?
        .json()
        .await
        .map_err(|error| error.to_string())?;
    let icon_url = project_response
        .get("data")
        .and_then(|project| project.get("logo"))
        .and_then(|logo| logo.get("thumbnailUrl").or_else(|| logo.get("url")))
        .and_then(Value::as_str);
    let mut icon_path = None;
    if let Some(url) = icon_url {
        let destination = instance_dir.join("icon.png");
        if download_to(&client, url, &destination).await.is_ok() {
            icon_path = Some(destination.to_string_lossy().into_owned());
        }
    }
    let metadata = serde_json::json!({
        "Id": format!("curseforge-{project_id}"),
        "Name": project_name,
        "Author": author,
        "Source": "CurseForge",
        "IconPath": icon_path.clone().unwrap_or_default(),
        "McVersion": minecraft,
        "Loader": loader,
        "LoaderVersion": loader_version,
        "VersionId": version_id,
        "InstallDate": "installed-by-vex-launcher"
    });
    fs::write(
        instance_dir.join("instance.json"),
        serde_json::to_string_pretty(&metadata).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    emit_progress(&app, operation, "Modpack instalado", 100, true);
    Ok(InstalledInstance {
        id: metadata["Id"].as_str().unwrap_or_default().to_owned(),
        name: metadata["Name"].as_str().unwrap_or_default().to_owned(),
        loader,
        mc_version: minecraft.to_owned(),
        version_id: metadata["VersionId"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
        profile_dir: instance_dir.to_string_lossy().into_owned(),
        icon_path,
        kind: String::from("modpack"),
        size_mb: (directory_size(&instance_dir) as f64 / 1_048_576.0 * 10.0).round() / 10.0,
        modified_unix: modified_unix(&instance_dir),
        last_played_unix: 0,
    })
}

#[tauri::command]
async fn launch_instance(
    app: tauri::AppHandle,
    version_id: String,
    profile_dir: String,
) -> Result<LaunchResult, String> {
    let operation = "launch-instance";
    emit_progress(&app, operation, "Preparando perfil", 3, false);
    let settings = read_settings();
    let game_dir = PathBuf::from(&settings.game_directory);
    let profile_dir = validated_instance_dir(&profile_dir)?;
    let direct_server = fs::read_to_string(profile_dir.join("instance.json"))
        .ok()
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .and_then(|metadata| {
            metadata
                .get("ServerAddress")
                .and_then(Value::as_str)
                .and_then(parse_server_address)
        });
    let logs_dir = storage_root().join("logs");
    fs::create_dir_all(&logs_dir).map_err(|error| error.to_string())?;
    let log_path = logs_dir.join("latest.log");
    fs::write(&log_path, format!("[Launcher] Preparando {version_id}\n"))
        .map_err(|error| error.to_string())?;
    let client = reqwest::Client::builder()
        .user_agent(VEX_USER_AGENT)
        .build()
        .map_err(|error| error.to_string())?;
    let microsoft_account = if settings.use_offline_profile {
        None
    } else {
        emit_progress(&app, operation, "Renovando sessão Microsoft", 8, false);
        append_log(&log_path, "Renovando sessão oficial Microsoft.");
        Some(refresh_microsoft_account(&client).await.map_err(|error| {
            format!("Não foi possível usar a conta Microsoft. Entre novamente nas configurações. {error}")
        })?)
    };
    append_log(&log_path, "Resolvendo metadados da versão.");
    let version = get_version_json(&client, &game_dir, &version_id).await?;
    emit_progress(&app, operation, "Metadados verificados", 16, false);
    let version_dir = game_dir.join("versions").join(&version_id);
    let client_jar = version_dir.join(format!("{version_id}.jar"));
    let client_url = version
        .get("downloads")
        .and_then(|downloads| downloads.get("client"))
        .and_then(|client| client.get("url"))
        .and_then(Value::as_str)
        .ok_or_else(|| String::from("Versão sem cliente para download."))?;
    append_log(&log_path, "Verificando cliente e bibliotecas.");
    download_to(&client, client_url, &client_jar).await?;
    emit_progress(&app, operation, "Preparando bibliotecas", 30, false);
    let natives_dir = version_dir.join("natives");
    fs::create_dir_all(&natives_dir).map_err(|error| error.to_string())?;
    let classpath =
        prepare_libraries(&client, &version, &game_dir, &natives_dir, &client_jar).await?;
    emit_progress(&app, operation, "Bibliotecas verificadas", 58, false);
    append_log(&log_path, "Verificando assets do Minecraft.");
    let (assets_dir, asset_index) = prepare_assets(&client, &version, &game_dir).await?;
    emit_progress(
        &app,
        operation,
        "Arquivos do Minecraft verificados",
        78,
        false,
    );
    fs::create_dir_all(profile_dir.join("mods")).map_err(|error| error.to_string())?;
    if settings.use_offline_profile {
        if let Some(skin) = settings.offline_skin_path.as_deref() {
            append_log(&log_path, "Aplicando skin offline global.");
            apply_offline_skin(Path::new(skin), &profile_dir)?;
        }
    }

    emit_progress(&app, operation, "Selecionando Java compatível", 86, false);
    append_log(&log_path, "Localizando Java compatível.");
    let required_java = version
        .get("javaVersion")
        .and_then(|java| java.get("majorVersion"))
        .and_then(Value::as_u64)
        .unwrap_or(8) as u32;
    let runtime = ensure_java_runtime(&app, required_java, operation, 86, 93).await?;
    append_log(
        &log_path,
        &format!("Usando Java {} em {}.", runtime.major, runtime.path),
    );

    let username = microsoft_account
        .as_ref()
        .map(|account| account.username.as_str())
        .unwrap_or_else(|| settings.offline_username.trim());
    let uuid = microsoft_account
        .as_ref()
        .map(|account| account.uuid.clone())
        .unwrap_or_else(|| offline_uuid(username));
    let access_token = microsoft_account
        .as_ref()
        .map(|account| account.access_token.as_str())
        .unwrap_or("0");
    let user_type = if microsoft_account.is_some() {
        "msa"
    } else {
        "offline"
    };
    append_log(
        &log_path,
        &format!(
            "Iniciando como {username} ({})",
            if microsoft_account.is_some() {
                "conta Microsoft"
            } else {
                "perfil offline"
            }
        ),
    );
    let main_class = version
        .get("mainClass")
        .and_then(Value::as_str)
        .ok_or_else(|| String::from("Versão sem classe principal."))?;
    let version_type = version
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("release");
    let separator = if cfg!(windows) { ";" } else { ":" };
    let classpath_text = classpath.join(separator);
    let game_dir_text = profile_dir.to_string_lossy().into_owned();
    let assets_dir_text = assets_dir.to_string_lossy().into_owned();
    let natives_dir_text = natives_dir.to_string_lossy().into_owned();
    let libraries_dir_text = game_dir.join("libraries").to_string_lossy().into_owned();
    let uuid_text = uuid.clone();
    let replacements = [
        ("${auth_player_name}", username),
        ("${version_name}", version_id.as_str()),
        ("${game_directory}", game_dir_text.as_str()),
        ("${assets_root}", assets_dir_text.as_str()),
        ("${assets_index_name}", asset_index.as_str()),
        ("${auth_uuid}", uuid_text.as_str()),
        ("${auth_access_token}", access_token),
        ("${clientid}", ""),
        ("${auth_xuid}", ""),
        ("${user_type}", user_type),
        ("${version_type}", version_type),
        ("${user_properties}", "{}"),
        ("${natives_directory}", natives_dir_text.as_str()),
        ("${launcher_name}", "VEXLauncher"),
        ("${launcher_version}", "0.6"),
        ("${classpath}", classpath_text.as_str()),
        ("${classpath_separator}", separator),
        ("${library_directory}", libraries_dir_text.as_str()),
    ];
    let extra_jvm: Vec<String> = version_arguments(&version, "jvm")
        .into_iter()
        .filter(|argument| {
            argument != "-cp"
                && !argument.contains("${classpath}")
                && !argument.contains("${natives_directory}")
                && !argument.contains("${launcher_name}")
                && !argument.contains("${launcher_version}")
        })
        .map(|argument| replace_launch_placeholders(&argument, &replacements))
        .collect();
    let extra_game: Vec<String> = version_arguments(&version, "game")
        .into_iter()
        .map(|argument| replace_launch_placeholders(&argument, &replacements))
        .collect();
    let legacy_game: Vec<String> = version
        .get("minecraftArguments")
        .and_then(Value::as_str)
        .map(|arguments| {
            arguments
                .split_whitespace()
                .map(|argument| replace_launch_placeholders(argument, &replacements))
                .collect()
        })
        .unwrap_or_default();
    let tweakers: Vec<String> = version
        .get("+tweakers")
        .or_else(|| version.get("tweakers"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .flat_map(|tweaker| [String::from("--tweakClass"), tweaker.to_owned()])
        .collect();
    let mut command = hidden_command(&runtime.path);
    if settings.mangohud_enabled {
        command.env("MANGOHUD", "1");
    }
    command
        .arg("-Xmx4G")
        .arg("-XX:+UseG1GC")
        .arg(format!(
            "-Djava.library.path={}",
            natives_dir.to_string_lossy()
        ))
        .arg("-Dminecraft.launcher.brand=VEXLauncher")
        .arg("-Dminecraft.launcher.version=0.6")
        .args(extra_jvm);
    command.arg("-cp").arg(classpath_text).arg(main_class);
    if legacy_game.is_empty() {
        command
            .arg("--username")
            .arg(username)
            .arg("--version")
            .arg(&version_id)
            .arg("--gameDir")
            .arg(&profile_dir)
            .arg("--assetsDir")
            .arg(&assets_dir)
            .arg("--assetIndex")
            .arg(asset_index)
            .arg("--uuid")
            .arg(uuid)
            .arg("--accessToken")
            .arg(access_token)
            .arg("--userType")
            .arg(user_type)
            .arg("--versionType")
            .arg(version_type)
            .args(extra_game);
    } else {
        command.args(legacy_game);
    }
    if let Some((host, port)) = direct_server {
        append_log(
            &log_path,
            &format!("Conectando automaticamente a {host}:{port}."),
        );
        command
            .arg("--server")
            .arg(host)
            .arg("--port")
            .arg(port.to_string());
    }
    command
        .args(tweakers)
        .current_dir(&game_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    append_log(&log_path, "Iniciando processo Java.");
    emit_progress(&app, operation, "Iniciando Minecraft", 96, false);
    let mut process = command.spawn().map_err(|error| error.to_string())?;
    let pid = process.id();
    let metadata_path = profile_dir.join("instance.json");
    if let Ok(raw) = fs::read_to_string(&metadata_path) {
        if let Ok(mut metadata) = serde_json::from_str::<Value>(&raw) {
            metadata["LastPlayedUnix"] = Value::from(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|duration| duration.as_secs())
                    .unwrap_or_default(),
            );
            let _ = fs::write(
                &metadata_path,
                serde_json::to_string_pretty(&metadata).unwrap_or(raw),
            );
        }
    }
    if let Some(stdout) = process.stdout.take() {
        spawn_log_reader(stdout, log_path.clone());
    }
    if let Some(stderr) = process.stderr.take() {
        spawn_log_reader(stderr, log_path.clone());
    }
    std::thread::spawn(move || {
        let _ = process.wait();
    });
    emit_progress(&app, operation, "Minecraft iniciado", 100, true);
    Ok(LaunchResult {
        pid,
        version_id,
        profile_dir: profile_dir.to_string_lossy().into_owned(),
        log_path: log_path.to_string_lossy().into_owned(),
    })
}

#[tauri::command]
fn save_offline_profile(
    username: String,
    skin_path: Option<String>,
) -> Result<LauncherSettings, String> {
    let clean_username = username.trim();
    let valid = (3..=16).contains(&clean_username.len())
        && clean_username
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_');
    if !valid {
        return Err(String::from(
            "Username must contain 3 to 16 letters, numbers, or underscores.",
        ));
    }

    let mut settings = read_settings();
    settings.offline_username = clean_username.to_owned();
    settings.offline_skin_path = skin_path.or(settings.offline_skin_path);
    settings.use_offline_profile = true;
    settings.onboarding_completed = true;
    write_settings(&settings)?;
    Ok(settings)
}

fn validate_offline_skin_png(bytes: &[u8]) -> Result<(), String> {
    const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
    if bytes.len() > 10 * 1024 * 1024 {
        return Err(String::from("A skin precisa ter no máximo 10 MB."));
    }
    if bytes.len() < 24 || &bytes[0..8] != PNG_SIGNATURE {
        return Err(String::from("A skin precisa ser um arquivo PNG válido."));
    }

    let width = u32::from_be_bytes(
        bytes[16..20]
            .try_into()
            .map_err(|_| String::from("PNG inválido."))?,
    );
    let height = u32::from_be_bytes(
        bytes[20..24]
            .try_into()
            .map_err(|_| String::from("PNG inválido."))?,
    );
    if width != 64 || (height != 64 && height != 32) {
        return Err(format!(
            "Dimensões inválidas: {width}x{height}. Use 64x64 ou 64x32."
        ));
    }
    Ok(())
}

fn normalized_skin_model(model: &str) -> String {
    match model.trim().to_ascii_lowercase().as_str() {
        "default" | "wide" => String::from("default"),
        "slim" => String::from("slim"),
        _ => String::from("auto-detect"),
    }
}

#[tauri::command]
fn list_saved_skins() -> Vec<OfflineSkinSummary> {
    list_skin_summaries()
}

#[tauri::command]
fn add_saved_skin(
    bytes: Vec<u8>,
    name: String,
    model: String,
) -> Result<OfflineSkinSummary, String> {
    validate_offline_skin_png(&bytes)?;
    let digest = Sha256::digest(&bytes);
    let id: String = digest[..12]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let skin_dir = offline_skin_library_dir();
    fs::create_dir_all(&skin_dir).map_err(|error| error.to_string())?;
    let skin_path = skin_dir.join(format!("{id}.png"));
    fs::write(&skin_path, bytes).map_err(|error| error.to_string())?;

    let mut skins = read_offline_skin_library();
    let clean_name = name.trim();
    let model = normalized_skin_model(&model);
    let created_unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default();
    if let Some(existing) = skins.iter_mut().find(|skin| skin.id == id) {
        if !clean_name.is_empty() {
            existing.name = clean_name.chars().take(48).collect();
        }
        existing.model = model.clone();
    } else {
        skins.push(StoredOfflineSkin {
            id: id.clone(),
            name: if clean_name.is_empty() {
                format!("Skin {}", skins.len() + 1)
            } else {
                clean_name.chars().take(48).collect()
            },
            path: skin_path.to_string_lossy().into_owned(),
            model: model.clone(),
            created_unix,
        });
    }
    write_offline_skin_library(&skins)?;

    let mut settings = read_settings();
    settings.offline_skin_path = Some(skin_path.to_string_lossy().into_owned());
    settings.use_offline_profile = true;
    settings.onboarding_completed = true;
    write_settings(&settings)?;
    let stored = skins
        .iter()
        .find(|skin| skin.id == id)
        .ok_or_else(|| String::from("A skin foi salva, mas não entrou na biblioteca."))?;
    Ok(skin_summary(stored, settings.offline_skin_path.as_deref()))
}

#[tauri::command]
fn activate_saved_skin(id: String) -> Result<OfflineSkinSummary, String> {
    let skins = read_offline_skin_library();
    let skin = skins
        .iter()
        .find(|skin| skin.id == id && Path::new(&skin.path).is_file())
        .ok_or_else(|| String::from("Skin salva não encontrada."))?;
    let mut settings = read_settings();
    settings.offline_skin_path = Some(skin.path.clone());
    settings.use_offline_profile = true;
    settings.onboarding_completed = true;
    write_settings(&settings)?;
    Ok(skin_summary(skin, settings.offline_skin_path.as_deref()))
}

#[tauri::command]
fn delete_saved_skin(id: String) -> Result<Vec<OfflineSkinSummary>, String> {
    let mut skins = read_offline_skin_library();
    let index = skins
        .iter()
        .position(|skin| skin.id == id)
        .ok_or_else(|| String::from("Skin salva não encontrada."))?;
    let removed = skins.remove(index);
    let removed_path = PathBuf::from(&removed.path);
    if removed_path.starts_with(offline_skin_library_dir()) {
        let _ = fs::remove_file(&removed_path);
    }
    let mut settings = read_settings();
    if settings.offline_skin_path.as_deref() == Some(removed.path.as_str()) {
        settings.offline_skin_path = skins.first().map(|skin| skin.path.clone());
    }
    write_settings(&settings)?;
    write_offline_skin_library(&skins)?;
    Ok(list_skin_summaries())
}

#[tauri::command]
fn save_offline_skin(bytes: Vec<u8>) -> Result<String, String> {
    add_saved_skin(
        bytes,
        String::from("Skin salva"),
        String::from("auto-detect"),
    )
    .map(|skin| skin.path)
}

#[tauri::command]
fn remove_offline_skin() -> Result<(), String> {
    let mut settings = read_settings();
    settings.offline_skin_path = None;
    write_settings(&settings)
}

#[tauri::command]
fn get_server_profile() -> ServerProfile {
    let mut profile = read_server_profile();
    if !profile.managed {
        let directory = PathBuf::from(&profile.directory);
        if directory.is_dir() {
            let target = resolved_server_target(&profile).ok();
            let detected_software = detected_server_software(&directory, target.as_deref());
            let detected_version = detected_server_version(&directory, target.as_deref());
            let mut changed = false;
            if matches!(profile.software.as_str(), "modded" | "unknown")
                && detected_software != "modded"
                && detected_software != "unknown"
            {
                profile.software = detected_software;
                changed = true;
            }
            if profile.version == "desconhecida" && detected_version != "desconhecida" {
                profile.version = detected_version;
                changed = true;
            }
            if changed {
                let _ = write_server_profile(&profile);
            }
        }
    }
    if profile.managed {
        let _ = fs::create_dir_all(&profile.directory);
    }
    profile
}

#[tauri::command]
fn list_server_profiles() -> Vec<ServerProfile> {
    read_server_registry().servers
}

fn ensure_server_stopped_for_profile_change() -> Result<(), String> {
    if server_status().running {
        return Err(String::from(
            "Pare o servidor antes de trocar ou remover o perfil ativo.",
        ));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn detected_external_server_pid(profile: &ServerProfile) -> Option<u32> {
    let expected_directory = fs::canonicalize(&profile.directory).ok()?;
    let own_pid = std::process::id();
    let mut candidates = Vec::new();
    for entry in fs::read_dir("/proc").ok()?.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|value| value.parse::<u32>().ok())
        else {
            continue;
        };
        if pid == own_pid {
            continue;
        }
        let process = entry.path();
        let Ok(cwd) = fs::read_link(process.join("cwd")) else {
            continue;
        };
        let cwd = fs::canonicalize(cwd).unwrap_or_else(|_| expected_directory.clone());
        if cwd != expected_directory {
            continue;
        }
        let Ok(command_line) = fs::read(process.join("cmdline")) else {
            continue;
        };
        let normalized = String::from_utf8_lossy(&command_line)
            .replace('\0', " ")
            .to_ascii_lowercase();
        let looks_like_server = normalized.contains(" -jar ")
            || normalized.contains(" nogui")
            || profile
                .start_target
                .as_deref()
                .is_some_and(|target| normalized.contains(&target.to_ascii_lowercase()));
        if !looks_like_server {
            continue;
        }
        let priority = if normalized.contains("java") && normalized.contains(" -jar ") {
            0
        } else {
            1
        };
        candidates.push((priority, pid));
    }
    candidates.sort();
    candidates.first().map(|(_, pid)| *pid)
}

#[cfg(not(target_os = "linux"))]
fn detected_external_server_pid(_profile: &ServerProfile) -> Option<u32> {
    None
}

fn server_port_online(port: u16) -> bool {
    TcpStream::connect_timeout(
        &SocketAddr::from(([127, 0, 0, 1], port)),
        Duration::from_millis(220),
    )
    .is_ok()
}

#[tauri::command]
fn select_server_profile(id: String) -> Result<ServerProfile, String> {
    let _operation = begin_server_operation()?;
    let id = id.trim();
    let mut registry = read_server_registry();
    let profile = registry
        .servers
        .iter()
        .find(|profile| profile.id == id)
        .cloned()
        .ok_or_else(|| String::from("O perfil de servidor escolhido não existe mais."))?;
    registry.active_id = profile.id.clone();
    persist_server_registry(&registry)?;
    Ok(profile)
}

#[tauri::command]
fn create_server_profile(
    name: String,
    version: String,
    software: String,
) -> Result<ServerProfile, String> {
    let _operation = begin_server_operation()?;
    let name = name.trim();
    let version = version.trim();
    let software = software.trim().to_ascii_lowercase();
    if name.is_empty() || version.is_empty() {
        return Err(String::from("Nome e versão do servidor são obrigatórios."));
    }
    if !matches!(software.as_str(), "vanilla" | "paper" | "fabric") {
        return Err(String::from("Escolha Vanilla, Paper ou Fabric."));
    }
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let digest = Sha256::digest(format!("{name}:{version}:{software}:{nonce}").as_bytes());
    let digest_hex = format!("{digest:x}");
    let short_id = &digest_hex[..12];
    let id = format!("managed-{short_id}");
    let profile = ServerProfile {
        id,
        name: name.to_owned(),
        version: version.to_owned(),
        software,
        directory: storage_root()
            .join("servers")
            .join(format!("{}-{short_id}", safe_directory_name(name)))
            .to_string_lossy()
            .into_owned(),
        ..ServerProfile::default()
    };
    save_server_profile_internal(profile)
}

#[tauri::command]
fn forget_server_profile(id: String) -> Result<Vec<ServerProfile>, String> {
    let _operation = begin_server_operation()?;
    let id = id.trim();
    let mut registry = read_server_registry();
    if let Some(profile) = registry.servers.iter().find(|profile| profile.id == id) {
        if active_server_runtime_record(profile).is_some()
            || detected_external_server_pid(profile).is_some()
            || server_port_online(profile.port)
        {
            return Err(String::from(
                "Pare este servidor antes de esquecer o perfil. Os outros servidores podem continuar ligados.",
            ));
        }
    }
    let previous_len = registry.servers.len();
    registry.servers.retain(|profile| profile.id != id);
    if registry.servers.len() == previous_len {
        return Err(String::from("O perfil de servidor não foi encontrado."));
    }
    if registry.active_id == id {
        registry.active_id = registry
            .servers
            .first()
            .map(|profile| profile.id.clone())
            .unwrap_or_default();
    }
    let registry = normalized_server_registry(registry);
    persist_server_registry(&registry)?;
    Ok(registry.servers)
}

fn inspect_server_directory(requested: &Path) -> Result<ServerProfile, String> {
    let canonical = fs::canonicalize(requested)
        .map_err(|_| String::from("A pasta selecionada não existe ou não pode ser lida."))?;
    if !canonical.is_dir() {
        return Err(String::from(
            "Selecione a pasta principal de um servidor Minecraft.",
        ));
    }

    #[cfg(windows)]
    let script =
        first_existing_server_file(&canonical, &["start.bat", "run.bat", "server.bat"], "bat")
            .or_else(|| {
                first_existing_server_file(
                    &canonical,
                    &["start.cmd", "run.cmd", "server.cmd"],
                    "cmd",
                )
            });
    #[cfg(not(windows))]
    let script = first_existing_server_file(&canonical, &["start.sh", "run.sh", "server.sh"], "sh");
    let jar = first_existing_server_file(
        &canonical,
        &[
            "server.jar",
            "paper.jar",
            "purpur.jar",
            "fabric-server-launch.jar",
            "forge.jar",
            "neoforge.jar",
        ],
        "jar",
    );
    let target = script.as_ref().or(jar.as_ref()).ok_or_else(|| {
        String::from(
            "O VEX não encontrou start.sh, run.sh, um script do Windows ou um JAR de servidor nesta pasta.",
        )
    })?;
    let properties = read_server_properties(&canonical);
    let name = canonical
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("Servidor importado")
        .to_owned();
    let profile = ServerProfile {
        id: format!(
            "imported-{}",
            &format!(
                "{:x}",
                Sha256::digest(canonical.to_string_lossy().as_bytes())
            )[..12]
        ),
        name,
        version: detected_server_version(&canonical, Some(target)),
        software: detected_server_software(&canonical, Some(target)),
        memory_gb: detected_memory_gb(script.as_deref()).clamp(1, 32),
        port: properties
            .get("server-port")
            .and_then(|value| value.parse().ok())
            .unwrap_or(25565),
        max_players: properties
            .get("max-players")
            .and_then(|value| value.parse().ok())
            .unwrap_or(20),
        motd: properties
            .get("motd")
            .cloned()
            .unwrap_or_else(|| String::from("Servidor Minecraft importado no VEX")),
        online_mode: properties
            .get("online-mode")
            .and_then(|value| value.parse().ok())
            .unwrap_or(true),
        gamemode: properties
            .get("gamemode")
            .cloned()
            .unwrap_or_else(|| String::from("survival")),
        difficulty: properties
            .get("difficulty")
            .cloned()
            .unwrap_or_else(|| String::from("normal")),
        directory: canonical.to_string_lossy().into_owned(),
        managed: false,
        launch_mode: if script.is_some() {
            String::from("script")
        } else {
            String::from("jar")
        },
        start_target: target
            .file_name()
            .and_then(|value| value.to_str())
            .map(str::to_owned),
        playit_enabled: false,
        heavy_storage_directory: None,
    };
    Ok(profile)
}

#[tauri::command]
fn import_server_directory(directory: String) -> Result<ServerProfile, String> {
    let _operation = begin_server_operation()?;
    let mut profile = inspect_server_directory(Path::new(directory.trim()))?;
    if let Some(existing) = read_server_registry()
        .servers
        .into_iter()
        .find(|existing| existing.id == profile.id)
    {
        profile.name = existing.name;
        profile.playit_enabled = existing.playit_enabled;
        profile.heavy_storage_directory = existing.heavy_storage_directory;
    }
    write_server_profile(&profile)?;
    Ok(profile)
}

fn save_server_profile_internal(mut profile: ServerProfile) -> Result<ServerProfile, String> {
    if profile.name.trim().is_empty() || profile.version.trim().is_empty() {
        return Err(String::from("Nome e versão do servidor são obrigatórios."));
    }
    profile.name = profile.name.trim().chars().take(96).collect();
    profile.version = profile.version.trim().chars().take(64).collect();
    if profile.id.is_empty()
        || profile.id.len() > 96
        || !profile
            .id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        return Err(String::from("Identificador interno de servidor inválido."));
    }
    let registry = read_server_registry();
    let existing = registry
        .servers
        .iter()
        .find(|existing| existing.id == profile.id)
        .cloned();
    if let Some(existing) = existing.as_ref() {
        if existing.managed != profile.managed {
            return Err(String::from(
                "A origem do perfil de servidor não pode ser alterada.",
            ));
        }
        profile.directory = existing.directory.clone();
        if !existing.managed {
            profile.version = existing.version.clone();
            profile.software = existing.software.clone();
            profile.port = existing.port;
            profile.max_players = existing.max_players;
            profile.motd = existing.motd.clone();
            profile.online_mode = existing.online_mode;
            profile.gamemode = existing.gamemode.clone();
            profile.difficulty = existing.difficulty.clone();
            profile.launch_mode = existing.launch_mode.clone();
            profile.start_target = existing.start_target.clone();
            if existing.launch_mode == "script" {
                profile.memory_gb = existing.memory_gb;
            }
        }
    } else if !profile.managed {
        return Err(String::from(
            "Use Encontrar servidor para registrar uma pasta externa.",
        ));
    }
    profile.memory_gb = profile.memory_gb.clamp(1, 32);
    profile.max_players = profile.max_players.clamp(1, 1000);
    profile.software = profile.software.trim().to_lowercase();
    profile.motd = profile.motd.replace(['\r', '\n'], " ");
    if profile.managed {
        if !profile.version.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-' | '+')
        }) {
            return Err(String::from(
                "A versão gerenciada contém caracteres não permitidos.",
            ));
        }
        if !matches!(profile.software.as_str(), "vanilla" | "paper" | "fabric") {
            return Err(String::from("Escolha Vanilla, Paper ou Fabric."));
        }
        profile.launch_mode = String::from("managed");
        profile.start_target = None;
        let root = storage_root().join("servers");
        let directory = PathBuf::from(&profile.directory);
        let has_parent_component = directory
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir));
        if !directory.starts_with(&root) || has_parent_component {
            profile.directory = root
                .join(safe_directory_name(&profile.name))
                .to_string_lossy()
                .into_owned();
        }
        if registry.servers.iter().any(|existing| {
            existing.id != profile.id
                && Path::new(&existing.directory) == Path::new(&profile.directory)
        }) {
            return Err(String::from("Outro perfil já usa essa pasta de servidor."));
        }
        fs::create_dir_all(&profile.directory).map_err(|error| error.to_string())?;
    } else {
        if !matches!(profile.launch_mode.as_str(), "script" | "jar") {
            return Err(String::from(
                "Modo de inicialização do servidor importado inválido.",
            ));
        }
        let canonical = fs::canonicalize(&profile.directory)
            .map_err(|_| String::from("A pasta importada do servidor não existe mais."))?;
        if !canonical.is_dir() {
            return Err(String::from(
                "A pasta importada deixou de ser um diretório.",
            ));
        }
        profile.directory = canonical.to_string_lossy().into_owned();
        let _ = resolved_server_target(&profile)?;
    }
    profile.heavy_storage_directory = profile
        .heavy_storage_directory
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| {
            let directory = fs::canonicalize(value).map_err(|_| {
                String::from("A pasta de dados pesados não existe ou não está montada.")
            })?;
            if !directory.is_dir() {
                return Err(String::from(
                    "Selecione uma pasta para os dados pesados do mapa.",
                ));
            }
            Ok(directory.to_string_lossy().into_owned())
        })
        .transpose()?;
    write_server_profile(&profile)?;
    Ok(profile)
}

#[tauri::command]
fn save_server_profile(profile: ServerProfile) -> Result<ServerProfile, String> {
    let _operation = begin_server_operation()?;
    ensure_server_stopped_for_profile_change()?;
    save_server_profile_internal(profile)
}

enum ServerJarHash {
    Sha1(String),
    Sha256(String),
}

fn valid_server_jar_signature(path: &Path) -> bool {
    let mut signature = [0_u8; 2];
    fs::File::open(path)
        .and_then(|mut file| file.read_exact(&mut signature))
        .is_ok()
        && &signature == b"PK"
}

fn server_jar_hash_matches(path: &Path, expected: &ServerJarHash) -> bool {
    let Ok(mut file) = fs::File::open(path) else {
        return false;
    };
    let mut sha1 = Sha1::new();
    let mut sha256 = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let Ok(read) = file.read(&mut buffer) else {
            return false;
        };
        if read == 0 {
            break;
        }
        sha1.update(&buffer[..read]);
        sha256.update(&buffer[..read]);
    }
    match expected {
        ServerJarHash::Sha1(expected) => {
            format!("{:x}", sha1.finalize()).eq_ignore_ascii_case(expected)
        }
        ServerJarHash::Sha256(expected) => {
            format!("{:x}", sha256.finalize()).eq_ignore_ascii_case(expected)
        }
    }
}

fn validate_server_jar_url(url: &str) -> Result<(), String> {
    let parsed = reqwest::Url::parse(url).map_err(|_| String::from("URL de servidor inválida."))?;
    let host = parsed.host_str().unwrap_or_default().to_ascii_lowercase();
    let allowed = matches!(
        host.as_str(),
        "launcher.mojang.com"
            | "piston-data.mojang.com"
            | "fill-data.papermc.io"
            | "meta.fabricmc.net"
    );
    if parsed.scheme() != "https" || !allowed {
        return Err(String::from(
            "O servidor precisa vir de um host oficial Mojang, PaperMC ou Fabric.",
        ));
    }
    Ok(())
}

async fn download_server_jar(
    client: &reqwest::Client,
    url: &str,
    destination: &Path,
    expected_size: Option<u64>,
    expected_hash: Option<ServerJarHash>,
) -> Result<PathBuf, String> {
    const MAX_SERVER_JAR_BYTES: u64 = 256 * 1024 * 1024;
    validate_server_jar_url(url)?;
    if destination.is_file()
        && fs::metadata(destination)
            .map(|metadata| metadata.len() <= MAX_SERVER_JAR_BYTES)
            .unwrap_or(false)
        && valid_server_jar_signature(destination)
        && expected_size.is_none_or(|size| {
            fs::metadata(destination)
                .map(|metadata| metadata.len() == size)
                .unwrap_or(false)
        })
        && expected_hash
            .as_ref()
            .is_none_or(|hash| server_jar_hash_matches(destination, hash))
    {
        return Ok(destination.to_path_buf());
    }
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let part = destination.with_extension("jar.part");
    let _ = fs::remove_file(&part);
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|error| error.to_string())?
        .error_for_status()
        .map_err(|error| error.to_string())?;
    if response.content_length().is_some_and(|size| {
        size > MAX_SERVER_JAR_BYTES || expected_size.is_some_and(|expected| expected != size)
    }) {
        return Err(String::from(
            "O tamanho do servidor não corresponde ao publicado pela fonte oficial.",
        ));
    }
    let result: Result<(), String> = async {
        let mut output = fs::File::create(&part).map_err(|error| error.to_string())?;
        let mut sha1 = Sha1::new();
        let mut sha256 = Sha256::new();
        let mut downloaded = 0_u64;
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|error| error.to_string())?;
            downloaded = downloaded.saturating_add(chunk.len() as u64);
            if downloaded > MAX_SERVER_JAR_BYTES
                || expected_size.is_some_and(|expected| downloaded > expected)
            {
                return Err(String::from(
                    "O download do servidor ultrapassou o limite seguro.",
                ));
            }
            output
                .write_all(&chunk)
                .map_err(|error| error.to_string())?;
            sha1.update(&chunk);
            sha256.update(&chunk);
        }
        output.flush().map_err(|error| error.to_string())?;
        drop(output);
        if expected_size.is_some_and(|expected| downloaded != expected) {
            return Err(String::from("O download do servidor terminou incompleto."));
        }
        let matches = match expected_hash.as_ref() {
            Some(ServerJarHash::Sha1(expected)) => {
                format!("{:x}", sha1.finalize()).eq_ignore_ascii_case(expected)
            }
            Some(ServerJarHash::Sha256(expected)) => {
                format!("{:x}", sha256.finalize()).eq_ignore_ascii_case(expected)
            }
            None => true,
        };
        if !matches {
            return Err(String::from(
                "O servidor não passou na verificação de integridade da fonte oficial.",
            ));
        }
        if !valid_server_jar_signature(&part) {
            return Err(String::from("O arquivo recebido não é um JAR válido."));
        }
        Ok(())
    }
    .await;
    if let Err(error) = result {
        let _ = fs::remove_file(&part);
        return Err(error);
    }
    if destination.exists() {
        fs::remove_file(destination).map_err(|error| error.to_string())?;
    }
    fs::rename(&part, destination).map_err(|error| error.to_string())?;
    Ok(destination.to_path_buf())
}

fn latest_stable_paper_build(builds: &Value) -> Option<&Value> {
    builds
        .as_array()?
        .iter()
        .filter(|item| item.get("channel").and_then(Value::as_str) == Some("STABLE"))
        .max_by_key(|item| item.get("id").and_then(Value::as_u64).unwrap_or_default())
}

async fn prepare_server_jar(
    client: &reqwest::Client,
    profile: &ServerProfile,
    version: &Value,
    directory: &Path,
) -> Result<PathBuf, String> {
    match profile.software.as_str() {
        "vanilla" => {
            let server = version
                .get("downloads")
                .and_then(|downloads| downloads.get("server"))
                .ok_or_else(|| {
                    String::from("Esta versão não oferece um servidor Vanilla para download.")
                })?;
            let url = server
                .get("url")
                .and_then(Value::as_str)
                .ok_or_else(|| String::from("A Mojang não informou o download do servidor."))?;
            let sha1 = server
                .get("sha1")
                .and_then(Value::as_str)
                .filter(|hash| {
                    hash.len() == 40 && hash.chars().all(|character| character.is_ascii_hexdigit())
                })
                .ok_or_else(|| String::from("A Mojang não informou um SHA-1 válido."))?;
            let size = server.get("size").and_then(Value::as_u64);
            let destination = directory.join(format!("server-vanilla-{}.jar", profile.version));
            download_server_jar(
                client,
                url,
                &destination,
                size,
                Some(ServerJarHash::Sha1(sha1.to_owned())),
            )
            .await
        }
        "paper" => {
            let builds: Value = client
                .get(format!(
                    "https://fill.papermc.io/v3/projects/paper/versions/{}/builds",
                    profile.version
                ))
                .send()
                .await
                .map_err(|error| error.to_string())?
                .error_for_status()
                .map_err(|error| error.to_string())?
                .json()
                .await
                .map_err(|error| error.to_string())?;
            let build = latest_stable_paper_build(&builds).ok_or_else(|| {
                    format!(
                        "Paper não possui uma build estável para Minecraft {}. O VEX não instala builds experimentais automaticamente.",
                        profile.version
                    )
                })?;
            let build_id = build
                .get("id")
                .and_then(Value::as_u64)
                .ok_or_else(|| String::from("A build estável do Paper não informou seu ID."))?;
            let download = build
                .get("downloads")
                .and_then(|downloads| downloads.get("server:default"))
                .ok_or_else(|| String::from("A build estável do Paper não publicou o servidor."))?;
            let url = download
                .get("url")
                .and_then(Value::as_str)
                .ok_or_else(|| String::from("O Paper não informou a URL da build estável."))?;
            let sha256 = download
                .get("checksums")
                .and_then(|checksums| checksums.get("sha256"))
                .and_then(Value::as_str)
                .filter(|hash| {
                    hash.len() == 64 && hash.chars().all(|character| character.is_ascii_hexdigit())
                })
                .ok_or_else(|| String::from("O Paper não informou um SHA-256 válido."))?;
            let size = download.get("size").and_then(Value::as_u64);
            let destination =
                directory.join(format!("server-paper-{}-{build_id}.jar", profile.version));
            download_server_jar(
                client,
                url,
                &destination,
                size,
                Some(ServerJarHash::Sha256(sha256.to_owned())),
            )
            .await
        }
        "fabric" => {
            let loaders: Value = client
                .get(format!(
                    "https://meta.fabricmc.net/v2/versions/loader/{}",
                    profile.version
                ))
                .send()
                .await
                .map_err(|error| error.to_string())?
                .error_for_status()
                .map_err(|error| error.to_string())?
                .json()
                .await
                .map_err(|error| error.to_string())?;
            let loader = loaders
                .as_array()
                .and_then(|items| {
                    items
                        .iter()
                        .find(|item| {
                            item.pointer("/loader/stable").and_then(Value::as_bool) == Some(true)
                        })
                        .or_else(|| items.first())
                })
                .and_then(|item| item.get("loader"))
                .and_then(|loader| loader.get("version"))
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    format!(
                        "Fabric não está disponível para Minecraft {}.",
                        profile.version
                    )
                })?;
            if !loader.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-' | '+')
            }) {
                return Err(String::from(
                    "O Fabric informou uma versão de loader inválida.",
                ));
            }
            let installers: Value = client
                .get("https://meta.fabricmc.net/v2/versions/installer")
                .send()
                .await
                .map_err(|error| error.to_string())?
                .error_for_status()
                .map_err(|error| error.to_string())?
                .json()
                .await
                .map_err(|error| error.to_string())?;
            let installer = installers
                .as_array()
                .and_then(|items| {
                    items
                        .iter()
                        .find(|item| item.get("stable").and_then(Value::as_bool) == Some(true))
                        .or_else(|| items.first())
                })
                .and_then(|item| item.get("version"))
                .and_then(Value::as_str)
                .ok_or_else(|| String::from("Instalador Fabric não encontrado."))?;
            if !installer.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-' | '+')
            }) {
                return Err(String::from(
                    "O Fabric informou uma versão de instalador inválida.",
                ));
            }
            let url = format!(
                "https://meta.fabricmc.net/v2/versions/loader/{}/{loader}/{installer}/server/jar",
                profile.version
            );
            let destination =
                directory.join(format!("server-fabric-{}-{loader}.jar", profile.version));
            download_server_jar(client, &url, &destination, None, None).await
        }
        _ => Err(String::from("Software de servidor inválido.")),
    }
}

fn server_status_for_profile(profile: ServerProfile) -> ServerStatus {
    let runtime = active_server_runtime_record(&profile);
    let external_pid = runtime
        .is_none()
        .then(|| detected_external_server_pid(&profile))
        .flatten();
    let port_online =
        runtime.is_none() && external_pid.is_none() && server_port_online(profile.port);
    let pid = runtime
        .as_ref()
        .and_then(|record| record.server_pid)
        .or(external_pid);
    let running = runtime.is_some() || external_pid.is_some() || port_online;
    let startup_log = if running {
        let native = PathBuf::from(&profile.directory)
            .join("logs")
            .join("latest.log");
        if native.is_file() {
            read_text_tail(&native, 256 * 1024)
        } else {
            read_text_tail(&server_log_path_for(&profile), 256 * 1024)
        }
    } else {
        String::new()
    };
    let startup_lower = startup_log.to_ascii_lowercase();
    let (ready, startup_percent, startup_label) = if !running {
        (false, 0, String::from("Servidor parado"))
    } else if server_port_online(profile.port) || startup_lower.contains("done (") {
        (true, 100, String::from("Servidor pronto para jogadores"))
    } else if startup_lower.contains("preparing spawn area") {
        (false, 82, String::from("Preparando área inicial do mundo"))
    } else if startup_lower.contains("preparing level")
        || startup_lower.contains("loading properties")
    {
        (false, 68, String::from("Carregando mundo e configurações"))
    } else if startup_lower.contains("loading minecraft")
        || startup_lower.contains("starting minecraft server")
    {
        (false, 38, String::from("Carregando Minecraft e mods"))
    } else {
        (false, 15, String::from("Processo Java iniciado"))
    };
    let (control_mode, recovered, detail) = if let Some(record) = runtime.as_ref() {
        (
            String::from("supervised"),
            record.started_unix + 8 < unix_seconds(),
            if record.status == "stopping" {
                String::from("Encerramento seguro em andamento.")
            } else {
                String::from("Servidor controlado pelo supervisor persistente do VEX.")
            },
        )
    } else if external_pid.is_some() {
        (
            String::from("external"),
            true,
            String::from(
                "Servidor aberto fora do VEX; status e encerramento seguro foram recuperados.",
            ),
        )
    } else if port_online {
        (
            String::from("observed"),
            true,
            format!(
                "A porta {} está ativa, mas o processo não pôde ser identificado.",
                profile.port
            ),
        )
    } else {
        (
            String::from("stopped"),
            false,
            String::from("Servidor parado."),
        )
    };
    ServerStatus {
        running,
        ready,
        startup_percent,
        startup_label,
        pid,
        control_mode,
        recovered,
        detail,
        log_path: server_log_path_for(&profile).to_string_lossy().into_owned(),
        metrics: server_metrics(pid),
        profile,
    }
}

#[tauri::command]
fn server_status() -> ServerStatus {
    server_status_for_profile(read_server_profile())
}

#[tauri::command]
fn list_server_runtime_statuses() -> Vec<ServerProfileRuntimeSummary> {
    read_server_registry()
        .servers
        .into_iter()
        .map(|profile| {
            let status = server_status_for_profile(profile);
            ServerProfileRuntimeSummary {
                profile_id: status.profile.id,
                running: status.running,
                pid: status.pid,
                control_mode: status.control_mode,
            }
        })
        .collect()
}

#[tauri::command]
async fn start_server(app: tauri::AppHandle) -> Result<ServerStatus, String> {
    let _operation = begin_server_operation()?;
    let operation = "start-server";
    emit_progress(&app, operation, "Preparando servidor", 5, false);
    let current = server_status();
    if current.running {
        return Ok(current);
    }

    let profile = save_server_profile_internal(read_server_profile())?;
    apply_pending_bluemap_state(&profile)?;
    if server_port_online(profile.port) {
        return Err(format!(
            "A porta {} já está em uso. O servidor pode estar aberto pelo terminal ou por outro programa; feche-o antes de iniciar pelo VEX.",
            profile.port
        ));
    }
    let directory = PathBuf::from(&profile.directory);
    let log_path = server_log_path_for(&profile);
    fs::create_dir_all(
        log_path
            .parent()
            .ok_or_else(|| String::from("Caminho de log inválido."))?,
    )
    .map_err(|error| error.to_string())?;
    {
        let _guard = server_log_write_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        fs::write(
            &log_path,
            format!(
                "[Launcher] Preparando servidor {} {}\n",
                profile.name, profile.version
            ),
        )
        .map_err(|error| error.to_string())?;
    }
    if profile.playit_enabled {
        append_log(&log_path, "Iniciando o serviço Playit antes do Minecraft.");
        let playit = start_playit()?;
        append_log(&log_path, &format!("Playit: {}", playit.detail));
    }

    let command = if profile.managed {
        fs::write(directory.join("eula.txt"), "eula=true\n").map_err(|error| error.to_string())?;
        let properties = format!(
            "server-port={}\nmax-players={}\nmotd={}\nonline-mode={}\ngamemode={}\ndifficulty={}\nallow-flight=true\nview-distance=10\nsimulation-distance=8\n",
            profile.port, profile.max_players, profile.motd, profile.online_mode, profile.gamemode, profile.difficulty
        );
        fs::write(directory.join("server.properties"), properties)
            .map_err(|error| error.to_string())?;

        let client = reqwest::Client::builder()
            .user_agent(VEX_USER_AGENT)
            .build()
            .map_err(|error| error.to_string())?;
        let game_dir = PathBuf::from(read_settings().game_directory);
        let version = get_version_json(&client, &game_dir, &profile.version).await?;
        emit_progress(&app, operation, "Versão do servidor verificada", 28, false);
        append_log(
            &log_path,
            &format!("Baixando/verificando servidor {}.", profile.software),
        );
        let server_jar = prepare_server_jar(&client, &profile, &version, &directory).await?;
        emit_progress(&app, operation, "Servidor preparado", 72, false);

        let required_java = version
            .get("javaVersion")
            .and_then(|java| java.get("majorVersion"))
            .and_then(Value::as_u64)
            .unwrap_or(17) as u32;
        let runtime = ensure_java_runtime(&app, required_java, operation, 74, 90).await?;
        append_log(&log_path, &format!("Iniciando com Java {}.", runtime.major));
        let mut command = hidden_command(&runtime.path);
        command
            .arg(format!("-Xms{}G", profile.memory_gb.min(2)))
            .arg(format!("-Xmx{}G", profile.memory_gb))
            .arg("-jar")
            .arg(server_jar)
            .arg("nogui");
        command
    } else if profile.launch_mode == "script" {
        let target = resolved_server_target(&profile)?;
        let extension = target
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        append_log(
            &log_path,
            &format!(
                "Executando script importado {} sem alterar os arquivos do servidor.",
                target.display()
            ),
        );
        if extension == "sh" {
            #[cfg(windows)]
            {
                return Err(String::from(
                    "Este servidor usa um script Linux (.sh). Importe um .bat/.cmd ou configure um JAR no Windows.",
                ));
            }
            #[cfg(not(windows))]
            {
                let mut command = hidden_command("bash");
                command.arg(target);
                command
            }
        } else {
            #[cfg(windows)]
            {
                let mut command = hidden_command("cmd.exe");
                command.arg("/C").arg(target);
                command
            }
            #[cfg(not(windows))]
            {
                return Err(String::from(
                    "Este servidor usa um script do Windows. No Linux, selecione uma pasta com start.sh/run.sh ou um JAR.",
                ));
            }
        }
    } else {
        let target = resolved_server_target(&profile)?;
        let runtime = detect_java_runtimes()
            .into_iter()
            .max_by_key(|runtime| runtime.major)
            .ok_or_else(|| {
                String::from(
                    "Java não foi encontrado. Instale o Java exigido pelo servidor ou crie um start.sh que aponte para ele.",
                )
            })?;
        append_log(
            &log_path,
            &format!(
                "Iniciando JAR importado com Java {} sem reescrever server.properties.",
                runtime.major
            ),
        );
        let mut command = hidden_command(&runtime.path);
        command
            .arg(format!("-Xms{}G", profile.memory_gb.min(2)))
            .arg(format!("-Xmx{}G", profile.memory_gb))
            .arg("-jar")
            .arg(target)
            .arg("nogui");
        command
    };
    emit_progress(&app, operation, "Iniciando supervisor seguro", 92, false);
    let runtime_directory = server_runtime_directory(&profile);
    let commands_directory = runtime_directory.join("commands");
    if commands_directory.is_dir() {
        fs::remove_dir_all(&commands_directory).map_err(|error| error.to_string())?;
    }
    fs::create_dir_all(&commands_directory).map_err(|error| error.to_string())?;
    let heartbeat_path = runtime_directory.join("heartbeat");
    write_runtime_heartbeat(&heartbeat_path)?;
    let spec = ServerSupervisorSpec {
        profile_id: profile.id.clone(),
        program: command.get_program().to_string_lossy().into_owned(),
        arguments: command
            .get_args()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect(),
        working_directory: directory.to_string_lossy().into_owned(),
        log_path: log_path.to_string_lossy().into_owned(),
        record_path: server_runtime_record_path(&profile)
            .to_string_lossy()
            .into_owned(),
        commands_directory: commands_directory.to_string_lossy().into_owned(),
        heartbeat_path: heartbeat_path.to_string_lossy().into_owned(),
        stop_when_vex_exits: true,
        startup_commands: server_map_status_for(&profile)
            .ok()
            .filter(|status| status.bluemap_enabled)
            .map(|_| vec![String::from("bluemap stop")])
            .unwrap_or_default(),
    };
    let spec_path = server_runtime_spec_path(&profile);
    replace_text_file(
        &spec_path,
        &serde_json::to_string_pretty(&spec).map_err(|error| error.to_string())?,
    )?;
    let executable = env::current_exe().map_err(|error| error.to_string())?;
    let mut supervisor = hidden_command(executable);
    supervisor
        .arg("--vex-server-supervisor")
        .arg(&spec_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut supervisor = supervisor.spawn().map_err(|error| error.to_string())?;
    let mut ready = false;
    for _ in 0..60 {
        if let Some(record) = read_server_runtime_record(&profile) {
            if record.status == "running" {
                ready = true;
                break;
            }
            if record.status == "failed" {
                return Err(record
                    .error
                    .unwrap_or_else(|| String::from("O supervisor não iniciou o servidor.")));
            }
        }
        if let Some(status) = supervisor.try_wait().map_err(|error| error.to_string())? {
            return Err(format!(
                "O supervisor encerrou antes do servidor iniciar (código {:?}).",
                status.code()
            ));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    if !ready {
        return Err(String::from(
            "O supervisor não confirmou a inicialização do servidor em 3 segundos.",
        ));
    }
    emit_progress(&app, operation, "Servidor iniciado", 100, true);
    Ok(server_status())
}

fn send_server_command_for_profile(profile: &ServerProfile, command: String) -> Result<(), String> {
    let runtime = active_server_runtime_record(profile).ok_or_else(|| {
        if detected_external_server_pid(profile).is_some() || server_port_online(profile.port) {
            String::from(
                "O servidor foi aberto fora do VEX. Reinicie-o uma vez pelo VEX para recuperar o envio de comandos persistente.",
            )
        } else {
            String::from("O servidor não está em execução.")
        }
    })?;
    queue_server_command(&runtime, &command)
}

#[tauri::command]
fn send_server_command(command: String) -> Result<(), String> {
    send_server_command_for_profile(&read_server_profile(), command)
}

#[tauri::command]
fn stop_server() -> Result<(), String> {
    let profile = read_server_profile();
    if let Some(runtime) = active_server_runtime_record(&profile) {
        return queue_server_command(&runtime, "stop");
    }
    if let Some(pid) = detected_external_server_pid(&profile) {
        #[cfg(target_os = "linux")]
        {
            let status = hidden_command("kill")
                .arg("-TERM")
                .arg(pid.to_string())
                .status()
                .map_err(|error| error.to_string())?;
            if status.success() {
                append_log(
                    &server_log_path_for(&profile),
                    "Encerramento seguro solicitado ao servidor encontrado fora do VEX.",
                );
                return Ok(());
            }
            return Err(String::from(
                "O servidor externo foi encontrado, mas não aceitou o encerramento.",
            ));
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = pid;
        }
    }
    if server_port_online(profile.port) {
        return Err(String::from(
            "Há um servidor nessa porta, mas o VEX não conseguiu identificar o processo com segurança.",
        ));
    }
    Ok(())
}

fn stop_all_supervised_servers() {
    for profile in read_server_registry().servers {
        if let Some(runtime) = active_server_runtime_record(&profile) {
            let _ = queue_server_command(&runtime, "stop");
        }
    }
}

#[tauri::command]
fn read_server_log() -> String {
    let _guard = server_log_write_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let profile = read_server_profile();
    if active_server_runtime_record(&profile).is_none()
        && detected_external_server_pid(&profile).is_some()
    {
        let native_log = PathBuf::from(&profile.directory)
            .join("logs")
            .join("latest.log");
        if native_log.is_file() {
            return read_text_tail(&native_log, SERVER_LOG_READ_BYTES);
        }
    }
    read_text_tail(&server_log_path_for(&profile), SERVER_LOG_READ_BYTES)
}

#[tauri::command]
fn clear_server_log() -> Result<(), String> {
    let _operation = begin_server_operation()?;
    ensure_server_stopped_for_profile_change()?;
    let _guard = server_log_write_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    fs::write(server_log_path(), "").map_err(|error| error.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[cfg(not(test))]
    if let Err(error) = persist_environment_storage_root() {
        eprintln!("O VEX não conseguiu memorizar o disco configurado: {error}");
    }
    let root = storage_root();
    if let Err(error) = ensure_storage_layout(&root) {
        eprintln!("O VEX não conseguiu preparar o armazenamento: {error}");
    }
    let webview_data = root.join("webview-data");
    let _ = fs::create_dir_all(&webview_data);
    std::env::set_var("WEBVIEW2_USER_DATA_FOLDER", webview_data);
    #[cfg(target_os = "linux")]
    {
        if std::env::var_os("WEBKIT_DISABLE_COMPOSITING_MODE").is_none() {
            std::env::set_var("WEBKIT_DISABLE_COMPOSITING_MODE", "1");
        }
        if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
            std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
        }
    }

    let builder = tauri::Builder::default();
    #[cfg(desktop)]
    let builder = builder.plugin(tauri_plugin_single_instance::init(
        |app, _arguments, _working_directory| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
        },
    ));

    builder
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            start_server_heartbeat_thread();
            #[cfg(desktop)]
            {
                if let Some(icon) = app.default_window_icon().cloned() {
                    let menu = tauri::menu::MenuBuilder::new(app)
                        .text("tray-show", "Abrir VEX Launcher")
                        .text("tray-stop-server", "Parar servidor")
                        .separator()
                        .text("tray-quit", "Sair do VEX")
                        .build()?;
                    let tray_result = tauri::tray::TrayIconBuilder::with_id("vex-main-tray")
                        .icon(icon)
                        .tooltip("VEX Launcher")
                        .menu(&menu)
                        .show_menu_on_left_click(false)
                        .temp_dir_path(storage_root().join("temp").join("tray-icon"))
                        .build(app);
                    match tray_result {
                        Ok(_) => TRAY_AVAILABLE.store(true, Ordering::Release),
                        Err(error) => {
                            TRAY_AVAILABLE.store(false, Ordering::Release);
                            eprintln!("O ícone da bandeja do VEX não pôde ser criado: {error}");
                        }
                    }
                }
            }
            Ok(())
        })
        .on_menu_event(|app, event| match event.id().as_ref() {
            "tray-show" => {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.unminimize();
                    let _ = window.set_focus();
                }
            }
            "tray-stop-server" => {
                let _ = stop_server();
            }
            "tray-quit" => {
                APP_EXPLICIT_EXIT.store(true, Ordering::Release);
                stop_all_supervised_servers();
                app.exit(0);
            }
            _ => {}
        })
        .on_window_event(|window, event| {
            if window.label() != "main" || APP_EXPLICIT_EXIT.load(Ordering::Acquire) {
                return;
            }
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                if TRAY_AVAILABLE.load(Ordering::Acquire) {
                    let _ = window.hide();
                } else {
                    let _ = window.minimize();
                }
            }
        })
        .on_tray_icon_event(|app, event| {
            #[cfg(desktop)]
            if let tauri::tray::TrayIconEvent::Click {
                button: tauri::tray::MouseButton::Left,
                button_state: tauri::tray::MouseButtonState::Up,
                ..
            } = event
            {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.unminimize();
                    let _ = window.set_focus();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            storage_status,
            list_storage_disks,
            select_storage_disk,
            get_launcher_settings,
            set_game_directory,
            set_runtime_preferences,
            open_path,
            open_url,
            get_microsoft_account,
            get_microsoft_skin_data_url,
            begin_microsoft_login,
            complete_microsoft_login,
            choose_offline_mode,
            use_microsoft_account,
            logout_microsoft_account,
            minimize_window,
            hide_window_to_tray,
            toggle_maximize_window,
            close_window,
            start_window_dragging,
            clear_launcher_cache,
            get_curseforge_status,
            save_curseforge_api_key,
            remove_curseforge_api_key,
            read_image_data_url,
            list_installed_instances,
            scan_minecraft_installations,
            create_instance,
            list_loader_game_versions,
            clone_instance,
            backup_instance,
            delete_instance,
            set_instance_icon,
            list_instance_content,
            remove_instance_content,
            detect_java_runtimes,
            launch_instance,
            read_latest_log,
            search_curseforge,
            get_curseforge_project_versions,
            get_curseforge_install_targets,
            install_curseforge_target,
            install_curseforge_modpack,
            get_modrinth_install_targets,
            install_modrinth_target,
            check_modpack_update,
            install_modrinth_modpack,
            save_offline_profile,
            list_saved_skins,
            add_saved_skin,
            activate_saved_skin,
            delete_saved_skin,
            save_offline_skin,
            remove_offline_skin,
            get_server_profile,
            list_server_profiles,
            select_server_profile,
            create_server_profile,
            forget_server_profile,
            import_server_directory,
            save_server_profile,
            server_status,
            list_server_runtime_statuses,
            start_server,
            send_server_command,
            stop_server,
            read_server_log,
            clear_server_log,
            playit_status,
            start_playit,
            stop_playit,
            repair_playit_access,
            setup_playit,
            list_server_content,
            set_server_content_enabled,
            export_server_client_pack,
            create_server_playable_instance,
            server_map_status,
            server_last_player_location,
            server_map_task_progress,
            set_bluemap_enabled,
            set_server_heavy_storage,
            preview_chunk_generation,
            start_chunk_generation,
            control_chunk_generation,
            render_bluemap,
            control_bluemap,
            configure_bluemap_area,
            install_server_map_stack
        ])
        .run(tauri::generate_context!())
        .expect("error while running VEX Launcher");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offline_uuid_is_stable_and_name_specific() {
        assert_eq!(offline_uuid("Player"), offline_uuid("Player"));
        assert_ne!(offline_uuid("Player"), offline_uuid("TestPlayer"));
    }

    #[test]
    fn settings_use_the_available_storage_root() {
        let settings = read_settings();
        assert_eq!(PathBuf::from(settings.storage_root), storage_root());
        assert!(!settings.game_directory.trim().is_empty());
        if let Some(path) = settings.offline_skin_path {
            assert!(Path::new(&path).is_absolute());
        }
    }

    #[test]
    fn probes_instances_and_java_without_panicking() {
        let _ = list_installed_instances();
        let _ = detect_java_runtimes();
    }

    #[test]
    fn inspects_imported_server_with_the_native_script_without_rewriting_it() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let directory = env::temp_dir().join(format!("vex-server-inspection-{unique}"));
        fs::create_dir_all(directory.join("mods")).expect("create fixture");
        fs::create_dir_all(directory.join("logs")).expect("create fixture logs");
        let properties = "server-port=25570\nmax-players=8\nmotd=Teste seguro\nonline-mode=false\ngamemode=creative\ndifficulty=hard\n";
        fs::write(
            directory.join("start.sh"),
            "#!/usr/bin/env bash\njava -Xms2G -Xmx6G -jar server.jar nogui\n",
        )
        .expect("write script");
        fs::write(
            directory.join("start.bat"),
            "@echo off\r\njava -Xms2G -Xmx6G -jar server.jar nogui\r\n",
        )
        .expect("write Windows script");
        fs::write(directory.join("server.properties"), properties).expect("write properties");
        fs::write(
            directory.join("logs").join("latest.log"),
            "[Server thread/INFO]: Starting minecraft server version 1.21.4\n",
        )
        .expect("write log");

        let profile = inspect_server_directory(&directory).expect("inspect server");
        assert!(!profile.managed);
        assert_eq!(profile.launch_mode, "script");
        #[cfg(windows)]
        assert_eq!(profile.start_target.as_deref(), Some("start.bat"));
        #[cfg(not(windows))]
        assert_eq!(profile.start_target.as_deref(), Some("start.sh"));
        assert_eq!(profile.version, "1.21.4");
        assert_eq!(profile.software, "modded");
        assert_eq!(profile.memory_gb, 6);
        assert_eq!(profile.port, 25570);
        assert_eq!(profile.max_players, 8);
        assert!(!profile.online_mode);
        assert_eq!(
            fs::read_to_string(directory.join("server.properties")).expect("read properties"),
            properties
        );
        assert!(!directory.join("eula.txt").exists());

        fs::remove_dir_all(directory).expect("remove fixture");
    }

    #[test]
    fn detects_fabric_loader_from_an_imported_start_script() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let directory = env::temp_dir().join(format!("vex-fabric-detection-{unique}"));
        fs::create_dir_all(directory.join("mods")).expect("create mods");
        fs::write(
            directory.join("start.sh"),
            "#!/bin/sh\njava -jar fabric-server-mc.1.21.1-loader.0.17.3.jar nogui\n",
        )
        .expect("write start script");
        let profile = inspect_server_directory(&directory).expect("inspect fabric server");
        assert_eq!(profile.software, "fabric");
        assert_eq!(
            client_loader_for_server(&profile, &directory).expect("resolve client loader"),
            "fabric"
        );
        fs::remove_dir_all(directory).expect("remove fixture");
    }

    #[test]
    fn writes_local_server_entry_for_playable_instance() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let directory = env::temp_dir().join(format!("vex-server-entry-{unique}"));
        fs::create_dir_all(&directory).expect("create instance fixture");
        write_local_server_entry(&directory, "Servidor teste", "localhost:25570")
            .expect("write servers.dat");
        let bytes = fs::read(directory.join("servers.dat")).expect("read servers.dat");
        assert!(bytes.starts_with(&[10, 0, 0, 9]));
        assert!(bytes
            .windows("localhost:25570".len())
            .any(|window| window == b"localhost:25570"));
        assert_eq!(
            parse_server_address("localhost:25570"),
            Some((String::from("localhost"), 25570))
        );
        assert!(parse_server_address("localhost;rm -rf:25570").is_none());
        fs::remove_dir_all(directory).expect("remove fixture");
    }

    #[test]
    fn configures_bluemap_in_temporary_directories() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let root = env::temp_dir().join(format!("vex-bluemap-config-{unique}"));
        let server = root.join("server");
        fs::create_dir_all(server.join("mods")).expect("create mods");
        fs::create_dir_all(server.join("world")).expect("create world");
        fs::write(server.join("server.properties"), "level-name=world\n")
            .expect("write properties");
        let profile_id = format!("fabric-test-{unique}");
        let profile = ServerProfile {
            id: profile_id.clone(),
            name: String::from("Servidor teste"),
            version: String::from("1.21.1"),
            software: String::from("fabric"),
            directory: server.to_string_lossy().into_owned(),
            managed: false,
            ..ServerProfile::default()
        };
        let port = configure_bluemap(&profile, &server).expect("configure bluemap");
        assert!((8100..8200).contains(&port));
        let config = server.join("config/bluemap");
        assert!(fs::read_to_string(config.join("core.conf"))
            .expect("read core")
            .contains("accept-download: true"));
        assert!(fs::read_to_string(config.join("webserver.conf"))
            .expect("read webserver")
            .contains(&format!("port: {port}")));
        assert!(config.join("maps/world.conf").is_file());
        let generated_maps = storage_root()
            .join("maps")
            .join(&profile_id)
            .join("bluemap/web/maps");
        assert!(generated_maps.is_dir());
        fs::remove_dir_all(root).expect("remove fixture");
        let _ = fs::remove_dir_all(storage_root().join("maps").join(profile_id));
    }

    #[test]
    fn normalizes_server_registry_ids_and_active_profile() {
        let mut first = ServerProfile::default();
        first.id.clear();
        first.directory = String::from("/fixture/first");
        let mut second = first.clone();
        second.directory = String::from("/fixture/second");
        let registry = normalized_server_registry(ServerRegistry {
            active_id: String::from("missing"),
            servers: vec![first, second],
        });
        assert_eq!(registry.servers.len(), 2);
        assert!(!registry.servers[0].id.is_empty());
        assert_ne!(registry.servers[0].id, registry.servers[1].id);
        assert_eq!(registry.active_id, registry.servers[0].id);
    }

    #[test]
    fn reads_only_the_bounded_tail_of_a_server_log() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let path = env::temp_dir().join(format!("vex-log-tail-{unique}.log"));
        fs::write(&path, "primeira linha\nsegunda linha\nterceira linha\n")
            .expect("write log fixture");
        let tail = read_text_tail(&path, 24);
        assert!(!tail.contains("primeira linha"));
        assert!(tail.ends_with("terceira linha\n"));
        fs::remove_file(path).expect("remove log fixture");
    }

    #[test]
    fn registry_json_recovers_from_a_valid_backup() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let directory = env::temp_dir().join(format!("vex-registry-recovery-{unique}"));
        fs::create_dir_all(&directory).expect("create recovery fixture");
        let path = directory.join("servers.json");
        fs::write(&path, "{invalid").expect("write corrupt registry");
        let registry = ServerRegistry::default();
        fs::write(
            sibling_work_file(&path, "bak").expect("backup path"),
            serde_json::to_string(&registry).expect("serialize registry"),
        )
        .expect("write backup");
        let recovered = read_json_with_recovery::<ServerRegistry>(&path).expect("recover registry");
        assert_eq!(recovered.active_id, registry.active_id);
        assert!(recovered.servers.is_empty());
        fs::remove_dir_all(directory).expect("remove recovery fixture");
    }

    #[test]
    fn empty_server_registry_stays_empty() {
        let registry = normalized_server_registry(ServerRegistry {
            active_id: String::from("stale-profile"),
            servers: Vec::new(),
        });
        assert!(registry.servers.is_empty());
        assert!(registry.active_id.is_empty());
    }

    #[test]
    fn extracts_map_progress_percentage_without_inventing_one() {
        assert_eq!(
            percentage_in_line("Waiting for task 'updating map world' (42.68%)"),
            Some(42.68)
        );
        assert_eq!(percentage_in_line("BlueMap is loading resources"), None);
        assert_eq!(percentage_in_line("progress: 1,410%"), Some(1.41));
        assert_eq!(percentage_in_line("invalid 120%"), None);
    }

    #[test]
    fn reads_player_position_from_minecraft_nbt() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let path = env::temp_dir().join(format!("vex-player-position-{unique}.dat"));
        let mut nbt = vec![10, 0, 0, 9, 0, 3, b'P', b'o', b's', 6, 0, 0, 0, 3];
        for value in [-803.25_f64, 67.0, -951.5] {
            nbt.extend_from_slice(&value.to_be_bytes());
        }
        let dimension = b"minecraft:overworld";
        nbt.extend_from_slice(&[8, 0, 9]);
        nbt.extend_from_slice(b"Dimension");
        nbt.extend_from_slice(&(dimension.len() as u16).to_be_bytes());
        nbt.extend_from_slice(dimension);
        nbt.push(0);
        let file = fs::File::create(&path).expect("create player fixture");
        let mut encoder = flate2::write::GzEncoder::new(file, flate2::Compression::fast());
        encoder.write_all(&nbt).expect("write player fixture");
        encoder.finish().expect("finish player fixture");
        let (position, dimension) = read_player_nbt_position(&path).expect("read player nbt");
        assert_eq!(position, [-803.25, 67.0, -951.5]);
        assert_eq!(dimension, "minecraft:overworld");
        fs::remove_file(path).expect("remove player fixture");
    }

    #[test]
    fn selects_only_the_latest_stable_paper_build() {
        let builds = serde_json::json!([
            {"id": 60, "channel": "ALPHA"},
            {"id": 41, "channel": "STABLE"},
            {"id": 43, "channel": "STABLE"}
        ]);
        assert_eq!(
            latest_stable_paper_build(&builds)
                .and_then(|build| build.get("id"))
                .and_then(Value::as_u64),
            Some(43)
        );
        assert!(latest_stable_paper_build(&serde_json::json!([
            {"id": 99, "channel": "ALPHA"}
        ]))
        .is_none());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_metrics_probe_current_process_without_panicking() {
        let metrics = server_metrics(Some(std::process::id()));
        assert!(metrics.system_memory_total_mb > 0.0);
        assert!(metrics.process_count >= 1);
    }

    #[test]
    fn classifies_fabric_server_only_mod_from_jar_metadata() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let directory = env::temp_dir().join(format!("vex-mod-side-{unique}"));
        fs::create_dir_all(&directory).expect("create fixture");
        let jar = directory.join("server-helper.jar");
        let file = fs::File::create(&jar).expect("create jar");
        let mut archive = zip::ZipWriter::new(file);
        archive
            .start_file(
                "fabric.mod.json",
                zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Deflated),
            )
            .expect("start metadata");
        archive
            .write_all(br#"{"schemaVersion":1,"id":"helper","environment":"server"}"#)
            .expect("write metadata");
        archive.finish().expect("finish jar");
        assert_eq!(jar_mod_side(&jar), "server-only");
        fs::remove_dir_all(directory).expect("remove fixture");
    }

    #[test]
    fn playable_instance_payload_excludes_server_only_and_sensitive_files() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let root = env::temp_dir().join(format!("vex-client-payload-{unique}"));
        let server = root.join("server");
        let instance = root.join("instance");
        fs::create_dir_all(server.join("mods")).expect("create mods");
        fs::create_dir_all(server.join("config")).expect("create config");
        let create_mod = |name: &str, environment: &str| {
            let file = fs::File::create(server.join("mods").join(name)).expect("create mod");
            let mut archive = zip::ZipWriter::new(file);
            archive
                .start_file(
                    "fabric.mod.json",
                    zip::write::SimpleFileOptions::default()
                        .compression_method(zip::CompressionMethod::Deflated),
                )
                .expect("start metadata");
            archive
                .write_all(
                    format!(
                        "{{\"schemaVersion\":1,\"id\":\"fixture\",\"environment\":\"{environment}\"}}"
                    )
                    .as_bytes(),
                )
                .expect("write metadata");
            archive.finish().expect("finish mod");
        };
        create_mod("client.jar", "client");
        create_mod("server.jar", "server");
        fs::write(server.join("config/client.json"), "{}\n").expect("write safe config");
        fs::write(server.join("config/auth-token.json"), "secret\n")
            .expect("write sensitive config");
        let profile = ServerProfile {
            id: String::from("payload-test"),
            name: String::from("Payload test"),
            version: String::from("1.21.1"),
            software: String::from("fabric"),
            directory: server.to_string_lossy().into_owned(),
            managed: false,
            ..ServerProfile::default()
        };
        let (included, skipped, _) =
            copy_server_client_payload(&profile, &server, &instance, false, true)
                .expect("copy client payload");
        assert_eq!(included, 2);
        assert!(skipped >= 2);
        assert!(instance.join("mods/client.jar").is_file());
        assert!(!instance.join("mods/server.jar").exists());
        assert!(instance.join("config/client.json").is_file());
        assert!(!instance.join("config/auth-token.json").exists());
        fs::remove_dir_all(root).expect("remove fixture");
    }

    #[test]
    fn microsoft_skin_urls_are_upgraded_and_restricted() {
        assert_eq!(
            normalized_minecraft_texture_url(
                "http://textures.minecraft.net/texture/0123456789abcdef"
            ),
            Some(String::from(
                "https://textures.minecraft.net/texture/0123456789abcdef"
            ))
        );
        assert!(normalized_minecraft_texture_url("https://example.com/texture/nope").is_none());
        assert!(
            normalized_minecraft_texture_url("https://textures.minecraft.net/other/nope").is_none()
        );
    }

    #[test]
    fn official_loader_versions_map_to_minecraft() {
        assert_eq!(
            official_loader_minecraft_version("forge", "1.21.1-52.1.14"),
            Some(String::from("1.21.1"))
        );
        assert_eq!(
            official_loader_minecraft_version("neoforge", "21.1.233"),
            Some(String::from("1.21.1"))
        );
        assert_eq!(
            official_loader_minecraft_version("neoforge", "26.1.2.75"),
            Some(String::from("26.1.2"))
        );
        assert_eq!(
            official_loader_minecraft_version("neoforge", "47.1.106"),
            Some(String::from("1.20.1"))
        );
        assert_eq!(
            maven_path("net.minecraftforge:forge:1.21.1-52.1.14:installer"),
            Some(String::from(
                "net/minecraftforge/forge/1.21.1-52.1.14/forge-1.21.1-52.1.14-installer.jar"
            ))
        );
        assert_eq!(
            maven_path("de.oceanlabs.mcp:mcp_config:1.21.1-20240808.132146@zip"),
            Some(String::from(
                "de/oceanlabs/mcp/mcp_config/1.21.1-20240808.132146/mcp_config-1.21.1-20240808.132146.zip"
            ))
        );
    }

    #[test]
    fn curseforge_loader_mapping_matches_official_api() {
        assert_eq!(curseforge_loader_type("Forge"), Some(1));
        assert_eq!(curseforge_loader_type("Fabric"), Some(4));
        assert_eq!(curseforge_loader_type("Quilt"), Some(5));
        assert_eq!(curseforge_loader_type("NeoForge"), Some(6));
    }
}
