import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import type {
  ChunkGenerationPreview,
  BlueMapAreaConfig,
  PlayitStatus,
  ServerContentItem,
  ServerMapStatus,
  ServerMapTaskProgress,
  ServerPackExportResult,
  ServerPlayableInstanceResult,
  ServerProfile,
  ServerProfileRuntimeSummary,
  ServerToolInstallResult,
  ServerStatus
} from "./serverTypes";

export const serverApi = {
  getProfile: () => invoke<ServerProfile>("get_server_profile"),
  listProfiles: () => invoke<ServerProfile[]>("list_server_profiles"),
  listRuntimeStatuses: () => invoke<ServerProfileRuntimeSummary[]>("list_server_runtime_statuses"),
  selectProfile: (id: string) => invoke<ServerProfile>("select_server_profile", { id }),
  createProfile: (name: string, version: string, software: string) => invoke<ServerProfile>("create_server_profile", { name, version, software }),
  forgetProfile: (id: string) => invoke<ServerProfile[]>("forget_server_profile", { id }),
  saveProfile: (profile: ServerProfile) => invoke<ServerProfile>("save_server_profile", { profile }),
  getStatus: () => invoke<ServerStatus>("server_status"),
  start: () => invoke<ServerStatus>("start_server"),
  stop: () => invoke("stop_server"),
  readLog: () => invoke<string>("read_server_log"),
  clearLog: () => invoke("clear_server_log"),
  sendCommand: (command: string) => invoke("send_server_command", { command }),
  getPlayitStatus: () => invoke<PlayitStatus>("playit_status"),
  startPlayit: () => invoke<PlayitStatus>("start_playit"),
  stopPlayit: () => invoke<PlayitStatus>("stop_playit"),
  repairPlayitAccess: () => invoke<PlayitStatus>("repair_playit_access"),
  setupPlayit: () => invoke<string>("setup_playit"),
  listContent: () => invoke<ServerContentItem[]>("list_server_content"),
  setContentEnabled: (path: string, enabled: boolean) => invoke<ServerContentItem[]>("set_server_content_enabled", { path, enabled }),
  exportClientPack: (includeUnknown: boolean, includeConfigs: boolean) => invoke<ServerPackExportResult>("export_server_client_pack", { includeUnknown, includeConfigs }),
  createPlayableInstance: (includeUnknown: boolean, includeConfigs: boolean) => invoke<ServerPlayableInstanceResult>("create_server_playable_instance", { includeUnknown, includeConfigs }),
  getMapStatus: () => invoke<ServerMapStatus>("server_map_status"),
  getLastPlayerLocation: (name?: string) => invoke<import("./serverTypes").ServerMapPlayer>("server_last_player_location", { name }),
  configureBlueMapArea: (world: string, x: number, z: number, radiusChunks: number) => invoke<BlueMapAreaConfig>("configure_bluemap_area", { world, x, z, radiusChunks }),
  getMapTaskProgress: () => invoke<ServerMapTaskProgress>("server_map_task_progress"),
  setBlueMapEnabled: (enabled: boolean) => invoke<ServerMapStatus>("set_bluemap_enabled", { enabled }),
  setHeavyStorage: (directory: string | null) => invoke<ServerProfile>("set_server_heavy_storage", { directory }),
  previewChunks: (world: string, shape: "square" | "circle", x: number, z: number, radius: number) => invoke<ChunkGenerationPreview>("preview_chunk_generation", { world, shape, x, z, radius }),
  startChunks: (world: string, shape: "square" | "circle", x: number, z: number, radius: number) => invoke<string>("start_chunk_generation", { world, shape, x, z, radius }),
  controlChunks: (action: "pause" | "continue" | "cancel" | "progress", world?: string) => invoke<string>("control_chunk_generation", { action, world }),
  renderBlueMap: (world: string, x: number, z: number, radius: number, threads: number, force: boolean) => invoke<string>("render_bluemap", { world, x, z, radius, threads, force }),
  controlBlueMap: (action: "status" | "pause" | "resume" | "tasks" | "cancel" | "reload" | "freeze" | "unfreeze" | "troubleshoot", world?: string, x?: number, z?: number) => invoke<string>("control_bluemap", { action, world, x, z }),
  installMapStack: (provider: "chunky" | "bluemap", acceptMojangDownload = false) => invoke<ServerToolInstallResult>("install_server_map_stack", { provider, acceptMojangDownload }),
  importDirectory: (directory: string) => invoke<ServerProfile>("import_server_directory", { directory }),
  openPath: (path: string) => invoke("open_path", { path }),
  openUrl: (url: string) => invoke("open_url", { url })
};

export async function chooseServerDirectory() {
  const selected = await open({
    directory: true,
    multiple: false,
    title: "Encontrar servidor do Minecraft"
  });
  if (Array.isArray(selected)) return selected[0] ?? null;
  return selected;
}

export async function chooseHeavyStorageDirectory() {
  const selected = await open({
    directory: true,
    multiple: false,
    title: "Escolher pasta no HD para mapas e dados pesados"
  });
  if (Array.isArray(selected)) return selected[0] ?? null;
  return selected;
}
