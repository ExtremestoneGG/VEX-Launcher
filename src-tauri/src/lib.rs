use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256, Sha512};
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::{
    collections::HashSet,
    env,
    fs::{self, OpenOptions},
    io::{BufRead, BufReader, Cursor, Read, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{Mutex, OnceLock},
    time::{SystemTime, UNIX_EPOCH},
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

const PREFERRED_STORAGE_ROOT: &str = r"D:\MineLauncher";
const CREATE_NO_WINDOW: u32 = 0x08000000;
const MICROSOFT_CLIENT_ID: &str = "00000000402b5328";
const MICROSOFT_REDIRECT_URI: &str = "https://login.live.com/oauth20_desktop.srf";

fn storage_root() -> PathBuf {
    if Path::new(r"D:\").is_dir() {
        return PathBuf::from(PREFERRED_STORAGE_ROOT);
    }
    #[cfg(not(windows))]
    {
        return env::var("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|_| {
                env::var("HOME").map(|home| PathBuf::from(home).join(".local").join("share"))
            })
            .unwrap_or_else(|_| env::temp_dir())
            .join("vex-launcher");
    }
    #[cfg(windows)]
    env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| env::temp_dir())
        .join("VEX Launcher")
}
fn default_game_directory() -> PathBuf {
    if Path::new(r"D:\").is_dir() {
        return PathBuf::from(r"D:\.minecraft");
    }
    #[cfg(not(windows))]
    {
        return env::var("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| storage_root())
            .join(".minecraft");
    }
    #[cfg(windows)]
    env::var("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| storage_root())
        .join(".minecraft")
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
    let mut command = Command::new(program);
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);
    command
}

#[derive(Serialize)]
struct StorageStatus {
    root: String,
    strategy: String,
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

#[derive(Debug, Clone, Serialize)]
struct MicrosoftAccountStatus {
    logged_in: bool,
    active: bool,
    username: String,
    uuid: String,
    skin_url: Option<String>,
    skin_data_url: Option<String>,
}

impl Default for MicrosoftAccountStatus {
    fn default() -> Self {
        Self {
            logged_in: false,
            active: false,
            username: String::new(),
            uuid: String::new(),
            skin_url: None,
            skin_data_url: None,
        }
    }
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

fn microsoft_skin_path() -> PathBuf {
    storage_root().join("profiles").join("microsoft-skin.png")
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

#[cfg(not(windows))]
fn protect_secret(_secret: &str) -> Result<String, String> {
    Err(String::from(
        "O armazenamento seguro da conta Microsoft ainda está disponível apenas no Windows.",
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

#[cfg(not(windows))]
fn unprotect_secret(_protected: &str) -> Result<String, String> {
    Err(String::from(
        "O armazenamento seguro da conta Microsoft ainda está disponível apenas no Windows.",
    ))
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
        .user_agent("VEXLauncher/0.6")
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
struct ServerProfile {
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
}

impl Default for ServerProfile {
    fn default() -> Self {
        Self {
            name: String::from("Meu servidor"),
            version: String::from("1.21.4"),
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
        }
    }
}

#[derive(Debug, Serialize)]
struct ServerStatus {
    running: bool,
    pid: Option<u32>,
    profile: ServerProfile,
    log_path: String,
}

struct ServerRuntime {
    child: Child,
    stdin: ChildStdin,
}

static SERVER_RUNTIME: OnceLock<Mutex<Option<ServerRuntime>>> = OnceLock::new();

fn server_runtime() -> &'static Mutex<Option<ServerRuntime>> {
    SERVER_RUNTIME.get_or_init(|| Mutex::new(None))
}

fn server_profile_path() -> PathBuf {
    storage_root().join("servers").join("server-profile.json")
}

fn server_log_path() -> PathBuf {
    storage_root().join("logs").join("server.log")
}

fn read_server_profile() -> ServerProfile {
    fs::read_to_string(server_profile_path())
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

fn write_server_profile(profile: &ServerProfile) -> Result<(), String> {
    let path = server_profile_path();
    fs::create_dir_all(
        path.parent()
            .ok_or_else(|| String::from("Caminho de servidor inválido."))?,
    )
    .map_err(|error| error.to_string())?;
    fs::write(
        path,
        serde_json::to_string_pretty(profile).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())
}

#[tauri::command]
fn storage_status() -> StorageStatus {
    StorageStatus {
        root: storage_root().to_string_lossy().into_owned(),
        strategy: String::from("user-selected-root"),
    }
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
    if !url.starts_with("https://") {
        return Err(String::from("Somente links HTTPS são permitidos."));
    }
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
        .user_agent("VEXLauncher/0.5")
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

#[tauri::command]
async fn complete_microsoft_login(code: String) -> Result<MicrosoftAccountStatus, String> {
    let client = reqwest::Client::builder()
        .user_agent("VEXLauncher/0.5")
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
    window.close().map_err(|error| error.to_string())
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

#[tauri::command]
fn list_installed_instances() -> Vec<InstalledInstance> {
    let settings = read_settings();
    let game_dir = PathBuf::from(&settings.game_directory);
    let mut instances = Vec::new();

    let modpacks_dir = game_dir.join("modpacks");
    if let Ok(entries) = fs::read_dir(&modpacks_dir) {
        for entry in entries.flatten().filter(|entry| entry.path().is_dir()) {
            let dir = entry.path();
            let json_path = dir.join("instance.json");
            let Ok(raw) = fs::read_to_string(json_path) else {
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
                    .unwrap_or("modpack")
                    .to_owned(),
                mc_version: json
                    .get("McVersion")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                version_id,
                profile_dir: dir.to_string_lossy().into_owned(),
                icon_path: json
                    .get("IconPath")
                    .and_then(Value::as_str)
                    .filter(|path| Path::new(path).exists())
                    .map(str::to_owned),
                kind: String::from("modpack"),
                size_mb: (directory_size(&dir) as f64 / 1_048_576.0 * 10.0).round() / 10.0,
                modified_unix: modified_unix(&dir),
                last_played_unix: json
                    .get("LastPlayedUnix")
                    .and_then(Value::as_u64)
                    .unwrap_or_default(),
            });
        }
    }

    let custom_instances_dir = game_dir.join("instances");
    if let Ok(entries) = fs::read_dir(&custom_instances_dir) {
        for entry in entries.flatten().filter(|entry| entry.path().is_dir()) {
            let dir = entry.path();
            let json_path = dir.join("instance.json");
            let Ok(raw) = fs::read_to_string(json_path) else {
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
                    .unwrap_or("vanilla")
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
                kind: String::from("instance"),
                size_mb: (directory_size(&dir) as f64 / 1_048_576.0 * 10.0).round() / 10.0,
                modified_unix: modified_unix(&dir),
                last_played_unix: json
                    .get("LastPlayedUnix")
                    .and_then(Value::as_u64)
                    .unwrap_or_default(),
            });
        }
    }

    instances.sort_by(|left, right| {
        right
            .last_played_unix
            .max(right.modified_unix)
            .cmp(&left.last_played_unix.max(left.modified_unix))
    });
    instances
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
    let version_id = if clean_loader == "fabric" {
        let client = reqwest::Client::builder()
            .user_agent("VEXLauncher/0.5")
            .build()
            .map_err(|error| error.to_string())?;
        let loaders: Value = client
            .get(format!(
                "https://meta.fabricmc.net/v2/versions/loader/{clean_version}"
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
            .and_then(|items| items.first())
            .and_then(|item| item.get("loader"))
            .and_then(|loader| loader.get("version"))
            .and_then(Value::as_str)
            .ok_or_else(|| format!("Fabric não está disponível para Minecraft {clean_version}."))?;
        install_fabric_profile(&client, &game_dir, clean_version, loader_version).await?
    } else if clean_loader == "quilt" {
        let client = reqwest::Client::builder()
            .user_agent("VEXLauncher/0.5")
            .build()
            .map_err(|error| error.to_string())?;
        install_quilt_profile(&client, &game_dir, clean_version).await?
    } else if clean_loader == "forge" || clean_loader == "neoforge" {
        install_official_loader_profile(
            &app,
            &game_dir,
            clean_version,
            &clean_loader,
            None,
            "create-instance",
            8,
            94,
        )
        .await?
    } else {
        clean_version.to_owned()
    };
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

#[tauri::command]
fn list_instance_content(
    profile_dir: String,
    category: String,
) -> Result<Vec<InstanceContent>, String> {
    let settings = read_settings();
    let game_dir = fs::canonicalize(&settings.game_directory)
        .unwrap_or_else(|_| PathBuf::from(&settings.game_directory));
    let profile = PathBuf::from(profile_dir);
    if !profile.starts_with(&game_dir) && !profile.starts_with(&settings.game_directory) {
        return Err(String::from(
            "A instância precisa ficar dentro da pasta do Minecraft.",
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
    content.sort_by(|left, right| right.modified_unix.cmp(&left.modified_unix));
    Ok(content)
}

#[tauri::command]
fn remove_instance_content(path: String) -> Result<(), String> {
    let settings = read_settings();
    let game_dir = fs::canonicalize(&settings.game_directory)
        .unwrap_or_else(|_| PathBuf::from(&settings.game_directory));
    let target = fs::canonicalize(path).map_err(|error| error.to_string())?;
    if !target.starts_with(&game_dir) || target == game_dir {
        return Err(String::from(
            "O arquivo precisa ficar dentro da pasta do Minecraft.",
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

fn validated_instance_dir(profile_dir: &str) -> Result<PathBuf, String> {
    let settings = read_settings();
    let game_dir = fs::canonicalize(&settings.game_directory)
        .unwrap_or_else(|_| PathBuf::from(&settings.game_directory));
    let target = fs::canonicalize(profile_dir).map_err(|error| error.to_string())?;
    if !target.starts_with(&game_dir)
        || target == game_dir
        || !target.join("instance.json").is_file()
    {
        return Err(String::from(
            "Instância inválida ou fora da pasta do Minecraft.",
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
    runtimes.sort_by(|left, right| right.major.cmp(&left.major));
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
        .user_agent("VEXLauncher/0.5")
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
    let parts: Vec<&str> = coordinate.split(':').collect();
    if parts.len() < 3 {
        return None;
    }
    let classifier = parts
        .get(3)
        .map(|value| format!("-{value}"))
        .unwrap_or_default();
    Some(format!(
        "{}/{}/{}/{}-{}{}.jar",
        parts[0].replace('.', "/"),
        parts[1],
        parts[2],
        parts[1],
        parts[2],
        classifier
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
        if let Some(artifact) = library
            .get("downloads")
            .and_then(|downloads| downloads.get("artifact"))
        {
            if let (Some(path), Some(url)) = (
                artifact.get("path").and_then(Value::as_str),
                artifact.get("url").and_then(Value::as_str),
            ) {
                let destination =
                    libraries_dir.join(path.replace('/', &std::path::MAIN_SEPARATOR.to_string()));
                let _ = download_to(client, url, &destination).await;
                if destination.is_file() {
                    classpath.push(destination.to_string_lossy().into_owned());
                }
            }
        } else if let Some(name) = library.get("name").and_then(Value::as_str) {
            if let Some(relative) = maven_path(name) {
                let destination = libraries_dir
                    .join(relative.replace('/', &std::path::MAIN_SEPARATOR.to_string()));
                let bases = [
                    library.get("url").and_then(Value::as_str).unwrap_or(""),
                    "https://maven.fabricmc.net/",
                    "https://maven.minecraftforge.net/",
                    "https://libraries.minecraft.net/",
                    "https://repo1.maven.org/maven2/",
                ];
                for base in bases.into_iter().filter(|base| !base.is_empty()) {
                    if download_to(
                        client,
                        &format!(
                            "{}{}",
                            base.trim_end_matches('/').to_owned() + "/",
                            relative
                        ),
                        &destination,
                    )
                    .await
                    .is_ok()
                    {
                        break;
                    }
                }
                if destination.is_file() {
                    classpath.push(destination.to_string_lossy().into_owned());
                }
            }
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
                    libraries_dir.join(path.replace('/', &std::path::MAIN_SEPARATOR.to_string()));
                if download_to(client, url, &destination).await.is_ok() {
                    let _ = extract_native(&destination, natives_dir);
                }
            }
        }
    }
    Ok(classpath)
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
        let mut log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_path)
            .ok();
        for line in BufReader::new(reader).lines().map_while(Result::ok) {
            if let Some(file) = log.as_mut() {
                let _ = writeln!(file, "{line}");
            }
        }
    });
}

fn append_log(log_path: &Path, message: &str) {
    if let Ok(mut log) = OpenOptions::new().create(true).append(true).open(log_path) {
        let _ = writeln!(log, "[Launcher] {message}");
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
        .user_agent("VEXLauncher/0.5")
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
    let server_dir = storage_root().join("servers");
    if !destination_dir.starts_with(&game_dir) && !destination_dir.starts_with(&server_dir) {
        return Err(String::from(
            "O destino precisa ficar dentro da pasta configurada do Minecraft ou dos servidores.",
        ));
    }
    fs::create_dir_all(&destination_dir).map_err(|error| error.to_string())?;
    let filename = Path::new(&target.filename)
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| String::from("Nome de arquivo inválido."))?;
    let client = reqwest::Client::builder()
        .user_agent("VEXLauncher/0.5")
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

async fn resolve_official_loader_version(
    client: &reqwest::Client,
    minecraft: &str,
    loader: &str,
    requested: Option<&str>,
) -> Result<String, String> {
    if let Some(requested) = requested.filter(|value| !value.trim().is_empty()) {
        return Ok(
            if loader == "forge" && !requested.starts_with(&format!("{minecraft}-")) {
                format!("{minecraft}-{requested}")
            } else {
                requested.to_owned()
            },
        );
    }
    let metadata_url = if loader == "forge" {
        "https://maven.minecraftforge.net/net/minecraftforge/forge/maven-metadata.xml"
    } else {
        "https://maven.neoforged.net/releases/net/neoforged/neoforge/maven-metadata.xml"
    };
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
    let versions = metadata_versions(&xml);
    let found = if loader == "forge" {
        let prefix = format!("{minecraft}-");
        versions
            .into_iter()
            .rev()
            .find(|version| version.starts_with(&prefix))
    } else {
        let (_, minor, patch) = minecraft_version_parts(minecraft);
        let prefix = format!("{minor}.{patch}.");
        versions
            .into_iter()
            .rev()
            .find(|version| version.starts_with(&prefix))
    };
    found.ok_or_else(|| format!("{loader} não está disponível para Minecraft {minecraft}."))
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
    candidates.sort_by(|left, right| right.0.cmp(&left.0));
    candidates.into_iter().next().map(|(_, id)| id)
}

async fn install_official_loader_profile(
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
        .user_agent("VEXLauncher/0.6")
        .build()
        .map_err(|error| error.to_string())?;
    emit_progress(
        app,
        operation,
        format!("Consultando versões do {loader}"),
        start,
        false,
    );
    let loader_version =
        resolve_official_loader_version(&client, minecraft, loader, requested_loader_version)
            .await?;
    if let Some(existing) = find_installed_loader_profile(game_dir, minecraft, loader) {
        if requested_loader_version.is_none()
            || existing
                .to_ascii_lowercase()
                .contains(&loader_version.to_ascii_lowercase())
        {
            return Ok(existing);
        }
    }
    let required_java = required_java_for_minecraft(minecraft);
    let runtime = ensure_java_runtime(
        app,
        required_java,
        operation,
        start.saturating_add(3),
        start.saturating_add(22),
    )
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
            &format!("Baixando instalador oficial do {loader}"),
            &client,
            &url,
            start.saturating_add(24),
            end.saturating_sub(25),
        )
        .await?;
        fs::write(&installer, bytes).map_err(|error| error.to_string())?;
    }
    fs::create_dir_all(game_dir).map_err(|error| error.to_string())?;
    emit_progress(
        app,
        operation,
        format!("Executando instalador oficial do {loader}"),
        end.saturating_sub(20),
        false,
    );
    let output = hidden_command(&runtime.path)
        .arg("-jar")
        .arg(&installer)
        .arg("--installClient")
        .arg(game_dir)
        .output()
        .map_err(|error| format!("Não foi possível executar o instalador do {loader}: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "O instalador oficial do {loader} falhou: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    emit_progress(app, operation, format!("{loader} instalado"), end, false);
    find_installed_loader_profile(game_dir, minecraft, loader).ok_or_else(|| {
        format!("O instalador do {loader} terminou, mas o perfil criado não foi encontrado.")
    })
}

#[tauri::command]
async fn install_modrinth_modpack(
    app: tauri::AppHandle,
    project_id: String,
    project_name: String,
    author: String,
    game_version: String,
) -> Result<InstalledInstance, String> {
    let operation = "install-modpack";
    emit_progress(&app, operation, "Consultando versões do modpack", 3, false);
    let settings = read_settings();
    let game_dir = PathBuf::from(&settings.game_directory);
    let client = reqwest::Client::builder()
        .user_agent("VEXLauncher/0.5")
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
    let instance_dir = game_dir.join("modpacks").join(&instance_name);
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
        let relative_path =
            PathBuf::from(relative.replace('/', &std::path::MAIN_SEPARATOR.to_string()));
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
        install_official_loader_profile(
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
        install_official_loader_profile(
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
        "IconPath": icon_path.clone().unwrap_or_default(),
        "McVersion": minecraft,
        "Loader": loader,
        "LoaderVersion": fabric_loader.or(quilt_loader).or(forge_loader).or(neoforge_loader).unwrap_or_default(),
        "VersionId": version_id,
        "InstallDate": "installed-by-mine-launcher"
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
            install_official_loader_profile(
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
    let profile_dir = PathBuf::from(profile_dir);
    let logs_dir = storage_root().join("logs");
    fs::create_dir_all(&logs_dir).map_err(|error| error.to_string())?;
    let log_path = logs_dir.join("latest.log");
    fs::write(&log_path, format!("[Launcher] Preparando {version_id}\n"))
        .map_err(|error| error.to_string())?;
    let client = reqwest::Client::builder()
        .user_agent("VEXLauncher/0.5")
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
        .args(extra_jvm)
        .arg("-cp")
        .arg(classpath_text)
        .arg(main_class)
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
        .args(extra_game)
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

#[tauri::command]
fn save_offline_skin(bytes: Vec<u8>) -> Result<String, String> {
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

    let skin_path = storage_root().join("profiles").join("offline_skin.png");
    let parent = skin_path
        .parent()
        .ok_or_else(|| String::from("Caminho de skin inválido."))?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    fs::write(&skin_path, bytes).map_err(|error| error.to_string())?;

    let mut settings = read_settings();
    settings.offline_skin_path = Some(skin_path.to_string_lossy().into_owned());
    settings.use_offline_profile = true;
    write_settings(&settings)?;
    Ok(skin_path.to_string_lossy().into_owned())
}

#[tauri::command]
fn remove_offline_skin() -> Result<(), String> {
    let mut settings = read_settings();
    if let Some(path) = settings.offline_skin_path.take() {
        let _ = fs::remove_file(path);
    }
    write_settings(&settings)
}

#[tauri::command]
fn get_server_profile() -> ServerProfile {
    let profile = read_server_profile();
    let _ = fs::create_dir_all(&profile.directory);
    let _ = write_server_profile(&profile);
    profile
}

#[tauri::command]
fn save_server_profile(mut profile: ServerProfile) -> Result<ServerProfile, String> {
    if profile.name.trim().is_empty() || profile.version.trim().is_empty() {
        return Err(String::from("Nome e versão do servidor são obrigatórios."));
    }
    profile.memory_gb = profile.memory_gb.clamp(1, 32);
    profile.max_players = profile.max_players.clamp(1, 1000);
    profile.software = profile.software.trim().to_lowercase();
    if !matches!(profile.software.as_str(), "vanilla" | "paper" | "fabric") {
        return Err(String::from("Escolha Vanilla, Paper ou Fabric."));
    }
    profile.motd = profile.motd.replace(['\r', '\n'], " ");
    let root = storage_root().join("servers");
    let directory = PathBuf::from(&profile.directory);
    if !directory.starts_with(&root) {
        profile.directory = root
            .join(safe_directory_name(&profile.name))
            .to_string_lossy()
            .into_owned();
    }
    fs::create_dir_all(&profile.directory).map_err(|error| error.to_string())?;
    write_server_profile(&profile)?;
    Ok(profile)
}

async fn prepare_server_jar(
    client: &reqwest::Client,
    profile: &ServerProfile,
    version: &Value,
    directory: &Path,
) -> Result<PathBuf, String> {
    let destination = directory.join(format!(
        "server-{}-{}.jar",
        profile.software, profile.version
    ));
    match profile.software.as_str() {
        "vanilla" => {
            let url = version
                .get("downloads")
                .and_then(|downloads| downloads.get("server"))
                .and_then(|server| server.get("url"))
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    String::from("Esta versão não oferece um servidor Vanilla para download.")
                })?;
            download_to(client, url, &destination).await?;
        }
        "paper" => {
            let builds: Value = client
                .get(format!(
                    "https://api.papermc.io/v2/projects/paper/versions/{}/builds",
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
            let build = builds
                .get("builds")
                .and_then(Value::as_array)
                .and_then(|items| items.last())
                .and_then(|item| {
                    item.get("build")
                        .and_then(Value::as_u64)
                        .or_else(|| item.as_u64())
                })
                .ok_or_else(|| {
                    format!(
                        "Paper não está disponível para Minecraft {}.",
                        profile.version
                    )
                })?;
            let filename = format!("paper-{}-{build}.jar", profile.version);
            let url = format!("https://api.papermc.io/v2/projects/paper/versions/{}/builds/{build}/downloads/{filename}", profile.version);
            download_to(client, &url, &destination).await?;
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
                .and_then(|items| items.first())
                .and_then(|item| item.get("loader"))
                .and_then(|loader| loader.get("version"))
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    format!(
                        "Fabric não está disponível para Minecraft {}.",
                        profile.version
                    )
                })?;
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
            let url = format!(
                "https://meta.fabricmc.net/v2/versions/loader/{}/{loader}/{installer}/server/jar",
                profile.version
            );
            download_to(client, &url, &destination).await?;
        }
        _ => return Err(String::from("Software de servidor inválido.")),
    }
    Ok(destination)
}

#[tauri::command]
fn server_status() -> ServerStatus {
    let mut runtime = server_runtime()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let exited = runtime
        .as_mut()
        .is_some_and(|server| server.child.try_wait().ok().flatten().is_some());
    if exited {
        *runtime = None;
    }
    ServerStatus {
        running: runtime.is_some(),
        pid: runtime.as_ref().map(|server| server.child.id()),
        profile: read_server_profile(),
        log_path: server_log_path().to_string_lossy().into_owned(),
    }
}

#[tauri::command]
async fn start_server(app: tauri::AppHandle) -> Result<ServerStatus, String> {
    let operation = "start-server";
    emit_progress(&app, operation, "Preparando servidor", 5, false);
    {
        let mut runtime = server_runtime()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let exited = runtime
            .as_mut()
            .is_some_and(|server| server.child.try_wait().ok().flatten().is_some());
        if exited {
            *runtime = None;
        }
        if runtime.is_some() {
            return Ok(ServerStatus {
                running: true,
                pid: runtime.as_ref().map(|server| server.child.id()),
                profile: read_server_profile(),
                log_path: server_log_path().to_string_lossy().into_owned(),
            });
        }
    }

    let profile = save_server_profile(read_server_profile())?;
    let directory = PathBuf::from(&profile.directory);
    let log_path = server_log_path();
    fs::create_dir_all(
        log_path
            .parent()
            .ok_or_else(|| String::from("Caminho de log inválido."))?,
    )
    .map_err(|error| error.to_string())?;
    fs::write(
        &log_path,
        format!(
            "[Launcher] Preparando servidor {} {}\n",
            profile.name, profile.version
        ),
    )
    .map_err(|error| error.to_string())?;
    fs::write(directory.join("eula.txt"), "eula=true\n").map_err(|error| error.to_string())?;
    let properties = format!(
        "server-port={}\nmax-players={}\nmotd={}\nonline-mode={}\ngamemode={}\ndifficulty={}\nallow-flight=true\nview-distance=10\nsimulation-distance=8\n",
        profile.port, profile.max_players, profile.motd, profile.online_mode, profile.gamemode, profile.difficulty
    );
    fs::write(directory.join("server.properties"), properties)
        .map_err(|error| error.to_string())?;

    let client = reqwest::Client::builder()
        .user_agent("VEXLauncher/0.5")
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
    emit_progress(&app, operation, "Iniciando processo do servidor", 92, false);
    let mut command = hidden_command(&runtime.path);
    command
        .arg(format!("-Xms{}G", profile.memory_gb.min(2)))
        .arg(format!("-Xmx{}G", profile.memory_gb))
        .arg("-jar")
        .arg(&server_jar)
        .arg("nogui")
        .current_dir(&directory)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|error| error.to_string())?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| String::from("Não foi possível abrir o console do servidor."))?;
    if let Some(stdout) = child.stdout.take() {
        spawn_log_reader(stdout, log_path.clone());
    }
    if let Some(stderr) = child.stderr.take() {
        spawn_log_reader(stderr, log_path.clone());
    }
    *server_runtime()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(ServerRuntime { child, stdin });
    emit_progress(&app, operation, "Servidor iniciado", 100, true);
    Ok(server_status())
}

#[tauri::command]
fn send_server_command(command: String) -> Result<(), String> {
    let clean = command.trim();
    if clean.is_empty() {
        return Ok(());
    }
    let mut runtime = server_runtime()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let server = runtime
        .as_mut()
        .ok_or_else(|| String::from("O servidor não está em execução."))?;
    writeln!(server.stdin, "{clean}").map_err(|error| error.to_string())?;
    server.stdin.flush().map_err(|error| error.to_string())
}

#[tauri::command]
fn stop_server() -> Result<(), String> {
    send_server_command(String::from("stop"))
}

#[tauri::command]
fn read_server_log() -> String {
    fs::read_to_string(server_log_path()).unwrap_or_default()
}

#[tauri::command]
fn clear_server_log() -> Result<(), String> {
    if server_status().running {
        return Err(String::from("Pare o servidor antes de limpar o console."));
    }
    fs::write(server_log_path(), "").map_err(|error| error.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let webview_data = storage_root().join("webview-data");
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

    tauri::Builder::default()
        .setup(|app| {
            #[cfg(desktop)]
            {
                let icon = app
                    .default_window_icon()
                    .cloned()
                    .ok_or_else(|| String::from("Ícone do VEX não encontrado."))?;
                tauri::tray::TrayIconBuilder::new()
                    .icon(icon)
                    .tooltip("VEX Launcher")
                    .build(app)?;
            }
            Ok(())
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
            create_instance,
            clone_instance,
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
            install_modrinth_modpack,
            save_offline_profile,
            save_offline_skin,
            remove_offline_skin,
            get_server_profile,
            save_server_profile,
            server_status,
            start_server,
            send_server_command,
            stop_server,
            read_server_log,
            clear_server_log
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
    fn official_loader_metadata_is_parsed() {
        let versions = metadata_versions(
            "<metadata><versioning><versions><version>1.20.1-47.4.20</version><version>21.1.200</version></versions></versioning></metadata>",
        );
        assert_eq!(versions, ["1.20.1-47.4.20", "21.1.200"]);
        assert_eq!(required_java_for_minecraft("1.20.1"), 17);
        assert_eq!(required_java_for_minecraft("1.21.1"), 21);
    }

    #[test]
    fn curseforge_loader_mapping_matches_official_api() {
        assert_eq!(curseforge_loader_type("Forge"), Some(1));
        assert_eq!(curseforge_loader_type("Fabric"), Some(4));
        assert_eq!(curseforge_loader_type("Quilt"), Some(5));
        assert_eq!(curseforge_loader_type("NeoForge"), Some(6));
    }
}
