export type ServerProfile = {
  id: string;
  name: string;
  version: string;
  software: string;
  memory_gb: number;
  port: number;
  max_players: number;
  motd: string;
  online_mode: boolean;
  gamemode: string;
  difficulty: string;
  directory: string;
  managed: boolean;
  launch_mode: string;
  start_target?: string;
  playit_enabled: boolean;
  heavy_storage_directory?: string;
};

export type ServerMetrics = {
  cpu_percent: number;
  memory_mb: number;
  system_memory_used_mb: number;
  system_memory_total_mb: number;
  process_count: number;
};

export type ServerStatus = {
  running: boolean;
  ready: boolean;
  startup_percent: number;
  startup_label: string;
  pid?: number;
  control_mode: "supervised" | "external" | "observed" | "stopped" | string;
  recovered: boolean;
  detail: string;
  profile: ServerProfile;
  log_path: string;
  metrics: ServerMetrics;
};

export type ServerProfileRuntimeSummary = {
  profile_id: string;
  running: boolean;
  pid?: number;
  control_mode: string;
};

export type PlayitStatus = {
  installed: boolean;
  running: boolean;
  service_running: boolean;
  authenticated: boolean;
  needs_setup: boolean;
  needs_group_access: boolean;
  detail: string;
};

export type ServerContentItem = {
  name: string;
  path: string;
  kind: "mod" | "plugin" | string;
  side: "server-only" | "client-only" | "client-required" | "unknown" | string;
  enabled: boolean;
  size_mb: number;
};

export type ServerPackExportResult = {
  path: string;
  included_files: number;
  skipped_files: number;
  warnings: string[];
};

export type ServerPlayableInstanceResult = {
  instance: {
    id: string;
    name: string;
    loader: string;
    mc_version: string;
    version_id: string;
    profile_dir: string;
    kind: string;
    size_mb: number;
  };
  included_files: number;
  skipped_files: number;
  warnings: string[];
  server_address: string;
};

export type ServerMapStatus = {
  provider?: string;
  providers: string[];
  web_url?: string;
  bluemap_web_url?: string;
  bluemap_online: boolean;
  port?: number;
  online: boolean;
  chunky_installed: boolean;
  bluemap_installed: boolean;
  bluemap_enabled: boolean;
  bluemap_pending_enabled?: boolean;
  bluemap_render_threads: number;
  bluemap_max_threads: number;
  bluemap_configured: boolean;
  bluemap_storage_safe: boolean;
  bluemap_storage_message: string;
  world_name: string;
  world_directory: string;
  heavy_storage_directory?: string;
  storage_safe: boolean;
  storage_message: string;
  world_generation_safe: boolean;
  world_generation_message: string;
  world_available_disk_gb: number;
  available_disk_gb: number;
};

export type ServerMapPlayer = {
  name: string;
  uuid: string;
  world: string;
  x?: number;
  y?: number;
  z?: number;
  yaw?: number;
  health?: number;
  armor?: number;
};

export type BlueMapAreaConfig = {
  map: string;
  x: number;
  z: number;
  radius_blocks: number;
  radius_chunks: number;
  config_path: string;
};

export type ServerMapTaskProgress = {
  operation: string;
  provider: string;
  active: boolean;
  determinate: boolean;
  percent?: number;
  label: string;
  detail: string;
  eta_seconds?: number;
  started_unix?: number;
};

export type ChunkGenerationPreview = {
  command: string;
  chunks: number;
  estimated_disk_gb: number;
  available_disk_gb: number;
  safe: boolean;
  message: string;
};

export type ServerToolInstallItem = {
  project: string;
  version: string;
  filename: string;
  destination: string;
  license: string;
};

export type ServerToolInstallResult = {
  installed: ServerToolInstallItem[];
  already_present: string[];
  source: string;
};

export type ServerSection = "overview" | "console" | "content" | "map" | "network" | "settings";

export const emptyServerMetrics: ServerMetrics = {
  cpu_percent: 0,
  memory_mb: 0,
  system_memory_used_mb: 0,
  system_memory_total_mb: 0,
  process_count: 0
};

export function joinLocalPath(root: string, ...parts: string[]) {
  const trimmedRoot = root.trim();
  const cleanParts = parts.map((part) => part.replace(/^[\\/]+|[\\/]+$/g, ""));
  if (trimmedRoot === "/") return `/${cleanParts.join("/")}`;
  const separator = trimmedRoot.includes("\\") && !trimmedRoot.includes("/") ? "\\" : "/";
  const cleanRoot = trimmedRoot.replace(/[\\/]+$/, "");
  return [cleanRoot || ".", ...cleanParts].join(separator);
}

export function createFallbackServerProfile(storageRoot: string): ServerProfile {
  return {
    id: "managed-default",
    name: "Meu servidor",
    version: "26.1.2",
    software: "vanilla",
    memory_gb: 4,
    port: 25565,
    max_players: 12,
    motd: "Servidor criado pelo VEX Launcher",
    online_mode: true,
    gamemode: "survival",
    difficulty: "normal",
    directory: joinLocalPath(storageRoot, "servers", "Meu servidor"),
    managed: true,
    launch_mode: "managed",
    playit_enabled: false,
    heavy_storage_directory: undefined
  };
}
