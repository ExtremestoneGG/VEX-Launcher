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
    time::UNIX_EPOCH,
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
    env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| env::temp_dir())
        .join("VEX Launcher")
}

fn default_game_directory() -> PathBuf {
    if Path::new(r"D:\").is_dir() {
        return PathBuf::from(r"D:\.minecraft");
    }
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
}

impl Default for MicrosoftAccountStatus {
    fn default() -> Self {
        Self {
            logged_in: false,
            active: false,
            username: String::new(),
            uuid: String::new(),
            skin_url: None,
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
    storage_root().join("profiles").join("microsoft-account.json")
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
            skin_url: account.skin_url,
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
fn open_path(path: String) -> Result<(), String> {
    let target = PathBuf::from(path.trim());
    if !target.exists() {
        return Err(String::from("O caminho não existe."));
    }
    let mut command = hidden_command("explorer.exe");
    if target.is_file() {
        command.arg("/select,");
    }
    command
        .arg(target)
        .spawn()
        .map_err(|error| error.to_string())?;
    Ok(())
}

#[tauri::command]
fn open_url(url: String) -> Result<(), String> {
    if !url.starts_with("https://") {
        return Err(String::from("Somente links HTTPS são permitidos."));
    }
    hidden_command("explorer.exe")
        .arg(url)
        .spawn()
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn microsoft_login_url() -> String {
    format!(
        "https://login.live.com/oauth20_authorize.srf?client_id={MICROSOFT_CLIENT_ID}&response_type=code&scope=XboxLive.signin%20offline_access&redirect_uri=https%3A%2F%2Flogin.live.com%2Foauth20_desktop.srf"
    )
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
    Ok(account)
}

#[tauri::command]
fn get_microsoft_account() -> MicrosoftAccountStatus {
    microsoft_account_status_value()
}

#[tauri::command]
fn begin_microsoft_login(app: tauri::AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window("microsoft-login") {
        window.set_focus().map_err(|error| error.to_string())?;
        return Ok(());
    }
    let app_for_navigation = app.clone();
    tauri::WebviewWindowBuilder::new(
        &app,
        "microsoft-login",
        tauri::WebviewUrl::External(
            microsoft_login_url()
                .parse()
                .map_err(|error| format!("URL de login inválida: {error}"))?,
        ),
    )
    .title("Entrar com Microsoft - VEX Launcher")
    .inner_size(520.0, 680.0)
    .min_inner_size(420.0, 560.0)
    .center()
    .on_navigation(move |url| {
        if url.as_str().starts_with(MICROSOFT_REDIRECT_URI) {
            if let Some(code) = url
                .query_pairs()
                .find_map(|(key, value)| (key == "code").then(|| value.into_owned()))
            {
                let _ = app_for_navigation.emit("microsoft-auth-code", code);
            } else {
                let _ = app_for_navigation.emit(
                    "microsoft-auth-error",
                    String::from("A Microsoft não retornou um código de autorização."),
                );
            }
            if let Some(window) = app_for_navigation.get_webview_window("microsoft-login") {
                let _ = window.close();
            }
            false
        } else {
            true
        }
    })
    .build()
    .map_err(|error| error.to_string())?;
    Ok(())
}

#[tauri::command]
async fn complete_microsoft_login(code: String) -> Result<MicrosoftAccountStatus, String> {
    let client = reqwest::Client::builder()
        .user_agent("VEXLauncher/0.3")
        .build()
        .map_err(|error| error.to_string())?;
    let (microsoft_access_token, refresh_token) =
        microsoft_token_from_code(&client, code.trim()).await?;
    let account =
        minecraft_account_from_microsoft_token(&client, &microsoft_access_token, refresh_token)
            .await?;
    write_microsoft_account(&account)?;
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

fn modified_unix(path: &Path) -> u64 {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

fn detect_loader(id: &str, raw_json: &str) -> String {
    let lowercase = format!("{id} {raw_json}").to_lowercase();
    for loader in ["fabric", "quilt", "neoforge", "forge"] {
        if lowercase.contains(loader) {
            return loader.to_owned();
        }
    }
    String::from("vanilla")
}

#[tauri::command]
fn list_installed_instances() -> Vec<InstalledInstance> {
    let settings = read_settings();
    let game_dir = PathBuf::from(&settings.game_directory);
    let mut instances = Vec::new();
    let mut represented_versions = HashSet::new();

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
            represented_versions.insert(version_id.clone());
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
            represented_versions.insert(version_id.clone());
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
                icon_path: None,
                kind: String::from("instance"),
                size_mb: (directory_size(&dir) as f64 / 1_048_576.0 * 10.0).round() / 10.0,
                modified_unix: modified_unix(&dir),
            });
        }
    }

    let versions_dir = game_dir.join("versions");
    if let Ok(entries) = fs::read_dir(&versions_dir) {
        for entry in entries.flatten().filter(|entry| entry.path().is_dir()) {
            let dir = entry.path();
            let id = entry.file_name().to_string_lossy().into_owned();
            if represented_versions.contains(&id) {
                continue;
            }
            let raw = fs::read_to_string(dir.join(format!("{id}.json"))).unwrap_or_default();
            let json = serde_json::from_str::<Value>(&raw).unwrap_or(Value::Null);
            let mc_version = json
                .get("inheritsFrom")
                .and_then(Value::as_str)
                .unwrap_or(&id)
                .to_owned();
            let loader = detect_loader(&id, &raw);
            instances.push(InstalledInstance {
                name: id.clone(),
                id: id.clone(),
                loader,
                mc_version,
                version_id: id.clone(),
                profile_dir: game_dir
                    .join("profiles")
                    .join(&id)
                    .to_string_lossy()
                    .into_owned(),
                icon_path: None,
                kind: String::from("version"),
                size_mb: (directory_size(&dir) as f64 / 1_048_576.0 * 10.0).round() / 10.0,
                modified_unix: modified_unix(&dir),
            });
        }
    }

    instances.sort_by(|left, right| right.modified_unix.cmp(&left.modified_unix));
    instances
}

#[tauri::command]
async fn create_instance(
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
    if clean_loader != "vanilla" && clean_loader != "fabric" {
        return Err(String::from("Escolha vanilla ou fabric."));
    }
    let settings = read_settings();
    let game_dir = PathBuf::from(&settings.game_directory);
    let instance_dir = game_dir.join("instances").join(&clean_name);
    fs::create_dir_all(&instance_dir).map_err(|error| error.to_string())?;
    let version_id = if clean_loader == "fabric" {
        let client = reqwest::Client::builder()
            .user_agent("VEXLauncher/0.3")
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

fn collect_java_in_dir(root: &Path, depth: usize, output: &mut Vec<PathBuf>) {
    if depth == 0 || !root.is_dir() {
        return;
    }
    for candidate in [
        root.join("bin").join("java.exe"),
        root.join("bin").join("javaw.exe"),
        root.join("java.exe"),
        root.join("javaw.exe"),
    ] {
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
    if let Ok(path) = env::var("PATH") {
        for dir in path.split(';') {
            for filename in ["java.exe", "javaw.exe"] {
                let candidate = PathBuf::from(dir).join(filename);
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
        .user_agent("VEXLauncher/0.3")
        .build()
        .map_err(|error| error.to_string())?;
    let assets: Value = client
        .get(format!(
            "https://api.adoptium.net/v3/assets/latest/{required_java}/hotspot?architecture=x64&image_type=jre&os=windows&vendor=eclipse"
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
            format!("Não foi possível localizar o Java {required_java} para Windows.")
        })?;
    let download_url = package.get("link").and_then(Value::as_str).ok_or_else(|| {
        format!("Não foi possível localizar o Java {required_java} para Windows.")
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
        format!("Instalando Java {required_java} no disco D"),
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
            Some("windows") => allowed = action == "allow",
            Some(_) if action == "disallow" => allowed = true,
            _ => {}
        }
    }
    allowed
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
            .and_then(|natives| natives.get("windows"))
            .and_then(Value::as_str)
            .unwrap_or("natives-windows")
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
        .user_agent("VEXLauncher/0.3")
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
        .user_agent("VEXLauncher/0.3")
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
        .user_agent("VEXLauncher/0.3")
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
    let loader = if fabric_loader.is_some() {
        "fabric"
    } else if dependencies.contains_key("quilt-loader") {
        "quilt"
    } else if dependencies.contains_key("neoforge") {
        "neoforge"
    } else if dependencies.contains_key("forge") {
        "forge"
    } else {
        "vanilla"
    };
    if loader != "fabric" && loader != "vanilla" {
        return Err(format!(
            "Instalação automática de {loader} será adicionada na próxima etapa."
        ));
    }

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
        "LoaderVersion": fabric_loader.unwrap_or_default(),
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
    fs::write(
        &log_path,
        format!("[Launcher] Preparando {version_id}\n"),
    )
    .map_err(|error| error.to_string())?;
    let client = reqwest::Client::builder()
        .user_agent("VEXLauncher/0.3")
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
    let mut command = hidden_command(&runtime.path);
    command
        .arg("-Xmx4G")
        .arg("-XX:+UseG1GC")
        .arg(format!(
            "-Djava.library.path={}",
            natives_dir.to_string_lossy()
        ))
        .arg("-Dminecraft.launcher.brand=VEXLauncher")
        .arg("-Dminecraft.launcher.version=0.3")
        .arg("-cp")
        .arg(classpath.join(separator))
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
        .current_dir(&game_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    append_log(&log_path, "Iniciando processo Java.");
    emit_progress(&app, operation, "Iniciando Minecraft", 96, false);
    let mut process = command.spawn().map_err(|error| error.to_string())?;
    let pid = process.id();
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
        .user_agent("VEXLauncher/0.3")
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

    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            storage_status,
            get_launcher_settings,
            set_game_directory,
            open_path,
            open_url,
            get_microsoft_account,
            begin_microsoft_login,
            complete_microsoft_login,
            choose_offline_mode,
            use_microsoft_account,
            logout_microsoft_account,
            minimize_window,
            toggle_maximize_window,
            close_window,
            start_window_dragging,
            clear_launcher_cache,
            read_image_data_url,
            list_installed_instances,
            create_instance,
            list_instance_content,
            remove_instance_content,
            detect_java_runtimes,
            launch_instance,
            read_latest_log,
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
}
