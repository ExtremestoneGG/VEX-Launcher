import {
  Activity,
  AlertTriangle,
  Archive,
  Box,
  Check,
  CheckCircle2,
  CircleHelp,
  Cpu,
  Download,
  ExternalLink,
  FileArchive,
  FolderOpen,
  Globe2,
  HardDrive,
  Layers3,
  LocateFixed,
  Map,
  Maximize2,
  Minimize2,
  PackagePlus,
  Pause,
  Play,
  Plus,
  Power,
  PowerOff,
  RefreshCw,
  RotateCcw,
  Save,
  Server,
  Settings,
  ShieldCheck,
  Square,
  TerminalSquare,
  Trash2,
  Wifi,
  WifiOff,
  X,
  Zap
} from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { chooseServerDirectory, serverApi } from "./serverApi";
import {
  createFallbackServerProfile,
  emptyServerMetrics,
  type ChunkGenerationPreview,
  type PlayitStatus,
  type ServerContentItem,
  type ServerMapStatus,
  type ServerMapTaskProgress,
  type ServerPackExportResult,
  type ServerPlayableInstanceResult,
  type ServerProfile,
  type ServerSection,
  type ServerToolInstallResult,
  type ServerStatus
} from "./serverTypes";

type ServerPageProps = {
  notify: (message: string) => void;
  storageRoot: string;
  onOpenDiscover: () => void;
  onOpenLibrary: () => void;
};

type BusyAction = "server" | "profile" | "import" | "save" | "command" | "playit" | "clear" | "content" | "content-toggle" | "export" | "instance" | "map" | "map-install" | null;

const sections: Array<{ id: ServerSection; label: string; icon: React.ReactNode }> = [
  { id: "overview", label: "Visão geral", icon: <Activity size={16} /> },
  { id: "console", label: "Console", icon: <TerminalSquare size={16} /> },
  { id: "content", label: "Conteúdo", icon: <PackagePlus size={16} /> },
  { id: "map", label: "Mapa", icon: <Map size={16} /> },
  { id: "network", label: "Rede", icon: <Globe2 size={16} /> },
  { id: "settings", label: "Configuração", icon: <Settings size={16} /> }
];

const emptyMapStatus: ServerMapStatus = {
  providers: [],
  online: false,
  chunky_installed: false,
  bluemap_installed: false,
  bluemap_enabled: false,
  bluemap_render_threads: 1,
  bluemap_max_threads: 1,
  bluemap_online: false,
  bluemap_configured: false,
  bluemap_storage_safe: false,
  bluemap_storage_message: "Escolha uma pasta no HD para os dados 3D.",
  world_name: "world",
  world_directory: "",
  storage_safe: false,
  storage_message: "Escolha uma pasta no HD para liberar operações pesadas.",
  world_generation_safe: false,
  world_generation_message: "O mundo ainda não foi validado.",
  world_available_disk_gb: 0,
  available_disk_gb: 0
};

const emptyMapTask: ServerMapTaskProgress = { operation: "idle", provider: "", active: false, determinate: false, label: "Nenhuma tarefa de mapa em andamento", detail: "Escolha uma região para renderizar ou gerar." };

function focusedMapUrl(baseUrl: string, focus: { world: string; x: number; z: number } | null) {
  if (!focus) return baseUrl;
  try {
    const url = new URL(baseUrl);
    url.hash = `${focus.world}:${Math.floor(focus.x)}:100:${Math.floor(focus.z)}:750:0:0:0:0:perspective`;
    return url.toString();
  } catch {
    return baseUrl;
  }
}

function serverWorldFromPlayerWorld(playerWorld: string, overworld: string) {
  const world = overworld || "world";
  if (playerWorld === "minecraft_overworld" || playerWorld === "minecraft:overworld" || playerWorld === "overworld") return world;
  if (playerWorld === "minecraft_the_nether" || playerWorld === "minecraft:the_nether" || playerWorld === "the_nether") return `${world}_nether`;
  if (playerWorld === "minecraft_the_end" || playerWorld === "minecraft:the_end" || playerWorld === "the_end") return `${world}_the_end`;
  return playerWorld;
}

function mergeProfile(profile: ServerProfile, fallback: ServerProfile): ServerProfile {
  return { ...fallback, ...profile };
}

function softwareLabel(software: string) {
  const labels: Record<string, string> = { vanilla: "Vanilla", paper: "Paper", fabric: "Fabric", forge: "Forge", neoforge: "NeoForge", modded: "Modificado" };
  return labels[software.toLowerCase()] ?? software;
}

function contentSideLabel(side: string) {
  const labels: Record<string, string> = {
    "server-only": "Somente servidor",
    "client-only": "Somente cliente",
    "client-required": "Cliente e servidor",
    unknown: "Lado desconhecido"
  };
  return labels[side] ?? side;
}

function formatMemory(megabytes: number) {
  if (!Number.isFinite(megabytes) || megabytes <= 0) return "0 MB";
  if (megabytes >= 1024) return `${(megabytes / 1024).toFixed(megabytes >= 10240 ? 0 : 1)} GB`;
  return `${megabytes.toFixed(0)} MB`;
}

function percentage(value: number, total: number) {
  if (!Number.isFinite(value) || !Number.isFinite(total) || total <= 0) return 0;
  return Math.min(100, Math.max(0, (value / total) * 100));
}

function IconButton({ label, children, onClick, disabled = false }: { label: string; children: React.ReactNode; onClick?: () => void; disabled?: boolean }) {
  return <button type="button" className="icon-button" aria-label={label} title={label} disabled={disabled} onClick={onClick}>{children}</button>;
}

function MetricCard({ icon, label, value, note, progress }: { icon: React.ReactNode; label: string; value: string; note: string; progress?: number }) {
  return <article className="server-metric-card">
    <span className="server-metric-icon">{icon}</span>
    <span className="server-metric-copy"><small>{label}</small><b>{value}</b><em>{note}</em></span>
    {typeof progress === "number" && <span className="server-meter" aria-hidden="true"><i style={{ width: `${Math.min(100, Math.max(0, progress))}%` }} /></span>}
  </article>;
}

export default function ServerPage({ notify, storageRoot, onOpenDiscover, onOpenLibrary }: ServerPageProps) {
  const fallback = useMemo(() => createFallbackServerProfile(storageRoot), [storageRoot]);
  const [section, setSection] = useState<ServerSection>("overview");
  const [createDialogOpen, setCreateDialogOpen] = useState(false);
  const [newServerName, setNewServerName] = useState("Novo servidor");
  const [newServerVersion, setNewServerVersion] = useState("26.1.2");
  const [newServerSoftware, setNewServerSoftware] = useState<"vanilla" | "paper" | "fabric">("paper");
  const [serverVersionOptions, setServerVersionOptions] = useState<string[]>([]);
  const [savedProfile, setSavedProfile] = useState<ServerProfile>(fallback);
  const [draftProfile, setDraftProfile] = useState<ServerProfile>(fallback);
  const [profiles, setProfiles] = useState<ServerProfile[]>([]);
  const [runningProfileIds, setRunningProfileIds] = useState<Set<string>>(() => new Set());
  const [libraryLoaded, setLibraryLoaded] = useState(false);
  const [draftDirty, setDraftDirty] = useState(false);
  const stoppedStatus = useCallback((profile: ServerProfile): ServerStatus => ({ running: false, ready: false, startup_percent: 0, startup_label: "Servidor parado", control_mode: "stopped", recovered: false, detail: "Servidor parado.", profile, log_path: "", metrics: emptyServerMetrics }), []);
  const [status, setStatus] = useState<ServerStatus>(() => stoppedStatus(fallback));
  const [playit, setPlayit] = useState<PlayitStatus>({ installed: false, running: false, service_running: false, authenticated: false, needs_setup: false, needs_group_access: false, detail: "Verificando instalação do playit..." });
  const [log, setLog] = useState("O servidor ainda não foi iniciado.");
  const [command, setCommand] = useState("");
  const [busy, setBusy] = useState<BusyAction>(null);
  const [connectionError, setConnectionError] = useState("");
  const [followingLog, setFollowingLog] = useState(true);
  const [contentItems, setContentItems] = useState<ServerContentItem[]>([]);
  const [contentError, setContentError] = useState("");
  const [includeUnknown, setIncludeUnknown] = useState(true);
  const [includeConfigs, setIncludeConfigs] = useState(false);
  const [exportResult, setExportResult] = useState<ServerPackExportResult | null>(null);
  const [playableInstanceResult, setPlayableInstanceResult] = useState<ServerPlayableInstanceResult | null>(null);
  const [mapStatus, setMapStatus] = useState<ServerMapStatus>(emptyMapStatus);
  const [mapWorld, setMapWorld] = useState("world");
  const [mapShape, setMapShape] = useState<"square" | "circle">("square");
  const [mapX, setMapX] = useState(0);
  const [mapZ, setMapZ] = useState(0);
  const [mapRadius, setMapRadius] = useState(1000);
  const [renderRadiusChunks, setRenderRadiusChunks] = useState(32);
  const [mapWorkspace, setMapWorkspace] = useState<"visualization" | "worldgen">("visualization");
  const [bluemapLoaded, setBluemapLoaded] = useState(false);
  const [mapTask, setMapTask] = useState<ServerMapTaskProgress>(emptyMapTask);
  const [chunkPreview, setChunkPreview] = useState<ChunkGenerationPreview | null>(null);
  const [mapMessage, setMapMessage] = useState("");
  const [mapInstallResult, setMapInstallResult] = useState<ServerToolInstallResult | null>(null);
  const [mapInstallResultProvider, setMapInstallResultProvider] = useState<"chunky" | "bluemap" | null>(null);
  const [mapFocus, setMapFocus] = useState<{ world: string; x: number; z: number } | null>(null);
  const [mapExpanded, setMapExpanded] = useState(false);
  const [blueMapThreads, setBlueMapThreads] = useState(1);
  const [forceBlueMapRender, setForceBlueMapRender] = useState(false);
  const dirtyRef = useRef(false);
  const profileEpochRef = useRef(0);
  const mountedRef = useRef(true);
  const consoleRef = useRef<HTMLPreElement>(null);

  const applyProfile = useCallback((profile: ServerProfile, forceDraft = false) => {
    const normalized = mergeProfile(profile, fallback);
    setSavedProfile(normalized);
    if (forceDraft || !dirtyRef.current) setDraftProfile(normalized);
    return normalized;
  }, [fallback]);

  const applyStatus = useCallback((nextStatus: ServerStatus, acceptProfile = true) => {
    const normalizedProfile = acceptProfile ? applyProfile(nextStatus.profile) : null;
    setStatus((current) => ({ ...nextStatus, profile: normalizedProfile ?? current.profile, metrics: { ...emptyServerMetrics, ...(nextStatus.metrics ?? {}) } }));
    setConnectionError("");
  }, [applyProfile]);

  const refreshProfiles = useCallback(async () => {
    try {
      const [items, runtimeStatuses] = await Promise.all([serverApi.listProfiles(), serverApi.listRuntimeStatuses()]);
      if (mountedRef.current) {
        setProfiles(items.map((profile) => mergeProfile(profile, fallback)));
        setRunningProfileIds(new Set(runtimeStatuses.filter((item) => item.running).map((item) => item.profile_id)));
      }
      return items;
    } catch (error) {
      if (mountedRef.current) setConnectionError(`Não foi possível carregar a biblioteca de servidores: ${String(error)}`);
      return [];
    }
  }, [fallback]);

  const resetProfileScopedState = useCallback(() => {
    setLog("O servidor ainda não foi iniciado.");
    setCommand("");
    setContentItems([]);
    setContentError("");
    setExportResult(null);
    setPlayableInstanceResult(null);
    setMapStatus(emptyMapStatus);
    setMapWorld("world");
    setChunkPreview(null);
    setMapMessage("");
    setMapInstallResult(null);
    setMapInstallResultProvider(null);
    setMapFocus(null);
    setMapExpanded(false);
    setBlueMapThreads(1);
    setForceBlueMapRender(false);
    setRenderRadiusChunks(32);
    setMapWorkspace("visualization");
    setBluemapLoaded(false);
    setMapTask(emptyMapTask);
  }, []);

  const refreshStatus = useCallback(async () => {
    const profileEpoch = profileEpochRef.current;
    try {
      const [nextStatus, runtimeStatuses] = await Promise.all([serverApi.getStatus(), serverApi.listRuntimeStatuses()]);
      if (mountedRef.current) {
        applyStatus(nextStatus, profileEpoch === profileEpochRef.current);
        setRunningProfileIds(new Set(runtimeStatuses.filter((item) => item.running).map((item) => item.profile_id)));
      }
    } catch (error) {
      if (mountedRef.current) setConnectionError(`Não foi possível consultar o servidor: ${String(error)}`);
    }
  }, [applyStatus]);

  const refreshPlayit = useCallback(async () => {
    try {
      const nextStatus = await serverApi.getPlayitStatus();
      if (mountedRef.current) setPlayit(nextStatus);
    } catch (error) {
      if (mountedRef.current) setPlayit({ installed: false, running: false, service_running: false, authenticated: false, needs_setup: false, needs_group_access: false, detail: `Integração indisponível: ${String(error)}` });
    }
  }, []);

  const refreshLog = useCallback(async () => {
    try {
      const content = await serverApi.readLog();
      if (mountedRef.current) setLog(content || "O console está vazio.");
    } catch (error) {
      if (mountedRef.current) setLog(`Não foi possível ler o console.\n${String(error)}`);
    }
  }, []);

  const refreshContent = useCallback(async () => {
    try {
      const items = await serverApi.listContent();
      if (mountedRef.current) {
        setContentItems(items);
        setContentError("");
      }
    } catch (error) {
      if (mountedRef.current) setContentError(String(error));
    }
  }, []);

  const refreshMapStatus = useCallback(async () => {
    try {
      const nextStatus = await serverApi.getMapStatus();
      if (mountedRef.current) {
        setMapStatus(nextStatus);
        setMapWorld((current) => current === "world" && nextStatus.world_name ? nextStatus.world_name : current);
        setBlueMapThreads((current) => current === 1 ? Math.max(1, nextStatus.bluemap_render_threads || 1) : Math.min(current, Math.max(1, nextStatus.bluemap_max_threads || 1)));
      }
    } catch (error) {
      if (mountedRef.current) setMapMessage(`Não foi possível consultar o mapa: ${String(error)}`);
    }
  }, []);

  const refreshMapTask = useCallback(async () => {
    try {
      const nextTask = await serverApi.getMapTaskProgress();
      if (mountedRef.current) setMapTask(nextTask);
    } catch {
      if (mountedRef.current) setMapTask(emptyMapTask);
    }
  }, []);

  useEffect(() => {
    mountedRef.current = true;
    void refreshProfiles().finally(() => mountedRef.current && setLibraryLoaded(true));
    void refreshPlayit();
    const playitInterval = window.setInterval(() => void refreshPlayit(), 3200);
    return () => {
      mountedRef.current = false;
      window.clearInterval(playitInterval);
    };
  }, [refreshPlayit, refreshProfiles]);

  useEffect(() => {
    if (!profiles.length) return;
    const profileEpoch = profileEpochRef.current;
    void serverApi.getProfile().then((profile) => {
      if (mountedRef.current && profileEpoch === profileEpochRef.current) {
        applyProfile(profile);
        void refreshProfiles();
      }
    }).catch((error) => mountedRef.current && setConnectionError(`Não foi possível carregar o servidor ativo: ${String(error)}`));
    void refreshStatus();
    const statusInterval = window.setInterval(() => void refreshStatus(), 1600);
    return () => window.clearInterval(statusInterval);
  }, [applyProfile, profiles.length, refreshProfiles, refreshStatus]);

  useEffect(() => {
    if (section !== "console" || !followingLog) return;
    void refreshLog();
    const interval = window.setInterval(() => void refreshLog(), status.running ? 1200 : 3200);
    return () => window.clearInterval(interval);
  }, [followingLog, refreshLog, section, status.running]);

  useEffect(() => {
    if (section !== "content") return;
    void refreshContent();
  }, [refreshContent, section, savedProfile.directory]);

  useEffect(() => {
    if (section !== "map") return;
    void refreshMapStatus();
    void refreshMapTask();
    const interval = window.setInterval(() => {
      void refreshMapStatus();
      void refreshMapTask();
    }, status.running ? 2400 : 7000);
    return () => window.clearInterval(interval);
  }, [refreshMapStatus, refreshMapTask, section, savedProfile.directory, status.running]);

  useEffect(() => {
    if (section !== "map" || mapWorkspace !== "visualization") setBluemapLoaded(false);
  }, [mapWorkspace, section]);

  useEffect(() => {
    setChunkPreview(null);
  }, [mapWorld, mapShape, mapX, mapZ, mapRadius]);

  useEffect(() => {
    if (!mapExpanded) return;
    const close = (event: KeyboardEvent) => event.key === "Escape" && setMapExpanded(false);
    window.addEventListener("keydown", close);
    return () => window.removeEventListener("keydown", close);
  }, [mapExpanded]);

  useEffect(() => {
    if (followingLog && consoleRef.current) consoleRef.current.scrollTop = consoleRef.current.scrollHeight;
  }, [followingLog, log]);

  const updateDraft = <Key extends keyof ServerProfile>(key: Key, value: ServerProfile[Key]) => {
    dirtyRef.current = true;
    setDraftDirty(true);
    setDraftProfile((current) => ({ ...current, [key]: value }));
  };

  const discardDraft = () => {
    dirtyRef.current = false;
    setDraftDirty(false);
    setDraftProfile(savedProfile);
  };

  const validateDraft = () => {
    if (!draftProfile.name.trim()) return "Informe um nome para o servidor.";
    if (!draftProfile.version.trim()) return "Informe uma versão do Minecraft.";
    if (!Number.isFinite(draftProfile.memory_gb) || draftProfile.memory_gb < 1 || draftProfile.memory_gb > 32) return "A memória deve ficar entre 1 e 32 GB.";
    if (!Number.isInteger(draftProfile.port) || draftProfile.port < 1 || draftProfile.port > 65535) return "A porta deve ficar entre 1 e 65535.";
    if (!Number.isInteger(draftProfile.max_players) || draftProfile.max_players < 1) return "O máximo de jogadores deve ser pelo menos 1.";
    return "";
  };

  const saveDraft = async () => {
    if (status.running) {
      notify("Pare o servidor antes de salvar a configuração.");
      return;
    }
    const validation = validateDraft();
    if (validation) {
      notify(validation);
      return;
    }
    setBusy("save");
    profileEpochRef.current += 1;
    try {
      const saved = await serverApi.saveProfile(draftProfile);
      const normalized = applyProfile(saved, true);
      dirtyRef.current = false;
      setDraftDirty(false);
      setStatus((current) => ({ ...current, profile: normalized }));
      await refreshProfiles();
      notify("Configuração do servidor salva");
    } catch (error) {
      notify(`Falha ao salvar servidor: ${String(error)}`);
    } finally {
      setBusy(null);
    }
  };

  const switchProfile = async (id: string) => {
    if (!id || id === savedProfile.id) return;
    if (draftDirty) {
      notify("Salve ou descarte as alterações antes de trocar de perfil.");
      return;
    }
    setBusy("profile");
    profileEpochRef.current += 1;
    try {
      const selected = await serverApi.selectProfile(id);
      const normalized = applyProfile(selected, true);
      dirtyRef.current = false;
      setDraftDirty(false);
      setStatus(stoppedStatus(normalized));
      resetProfileScopedState();
      await refreshProfiles();
      await refreshStatus();
      notify(`Servidor selecionado: ${normalized.name}`);
    } catch (error) {
      notify(`Não foi possível trocar de servidor: ${String(error)}`);
    } finally {
      setBusy(null);
    }
  };

  const openCreateServerDialog = () => {
    if (draftDirty) return;
    setNewServerName("Novo servidor");
    setNewServerVersion(savedProfile.version && savedProfile.version !== "desconhecida" ? savedProfile.version : "26.1.2");
    setNewServerSoftware(["vanilla", "paper", "fabric"].includes(savedProfile.software) ? savedProfile.software as "vanilla" | "paper" | "fabric" : "paper");
    setCreateDialogOpen(true);
    if (!serverVersionOptions.length) {
      void fetch("https://launchermeta.mojang.com/mc/game/version_manifest_v2.json")
        .then((response) => response.ok ? response.json() : Promise.reject(new Error("Manifesto indisponível")))
        .then((manifest: { versions?: Array<{ id: string; type: string }> }) => setServerVersionOptions((manifest.versions ?? []).filter((item) => item.type === "release").map((item) => item.id).slice(0, 80)))
        .catch(() => undefined);
    }
  };

  const createServer = async () => {
    if (draftDirty) return;
    const name = newServerName.trim();
    const version = newServerVersion.trim();
    if (!name || !version) {
      notify("Informe nome e versão do Minecraft.");
      return;
    }
    setBusy("profile");
    profileEpochRef.current += 1;
    try {
      const created = await serverApi.createProfile(name, version, newServerSoftware);
      const normalized = applyProfile(created, true);
      dirtyRef.current = false;
      setDraftDirty(false);
      setStatus(stoppedStatus(normalized));
      resetProfileScopedState();
      await refreshProfiles();
      setSection("settings");
      setCreateDialogOpen(false);
      notify("Servidor criado. Revise a versão e a configuração antes de iniciar.");
    } catch (error) {
      notify(`Não foi possível criar o servidor: ${String(error)}`);
    } finally {
      setBusy(null);
    }
  };

  const forgetCurrentServer = async () => {
    if (status.running || draftDirty) return;
    const confirmed = window.confirm(`Esquecer “${savedProfile.name}” no VEX?\n\nNenhum arquivo da pasta será apagado. Você poderá encontrá-la novamente depois.`);
    if (!confirmed) return;
    setBusy("profile");
    profileEpochRef.current += 1;
    try {
      const remaining = await serverApi.forgetProfile(savedProfile.id);
      if (!remaining.length) {
        setProfiles([]);
        applyProfile(fallback, true);
        dirtyRef.current = false;
        setDraftDirty(false);
        setStatus(stoppedStatus(fallback));
        resetProfileScopedState();
        setSection("overview");
        notify("Perfil esquecido pelo VEX; a pasta permaneceu intacta.");
        return;
      }
      const active = await serverApi.getProfile();
      const normalized = applyProfile(active, true);
      setProfiles(remaining.map((profile) => mergeProfile(profile, fallback)));
      dirtyRef.current = false;
      setDraftDirty(false);
      setStatus(stoppedStatus(normalized));
      resetProfileScopedState();
      await refreshStatus();
      notify("Perfil esquecido pelo VEX; a pasta permaneceu intacta.");
    } catch (error) {
      notify(`Não foi possível esquecer o perfil: ${String(error)}`);
    } finally {
      setBusy(null);
    }
  };

  const toggleServer = async () => {
    if (draftDirty) {
      notify("Salve ou descarte as alterações antes de iniciar o servidor.");
      return;
    }
    const wasRunning = status.running;
    setBusy("server");
    try {
      if (wasRunning) {
        await serverApi.stop();
        await refreshStatus();
      } else {
        const nextStatus = await serverApi.start();
        applyStatus(nextStatus);
      }
      notify(wasRunning ? "Comando de desligamento enviado" : "Servidor iniciando");
    } catch (error) {
      notify(`Falha no servidor: ${String(error)}`);
    } finally {
      setBusy(null);
    }
  };

  const importServer = async () => {
    setBusy("import");
    try {
      const directory = await chooseServerDirectory();
      if (!directory) return;
      const confirmed = window.confirm(`Encontrar e analisar o servidor nesta pasta?\n\n${directory}\n\nO VEX não apagará os arquivos existentes.`);
      if (!confirmed) return;
      profileEpochRef.current += 1;
      const imported = await serverApi.importDirectory(directory);
      const normalized = applyProfile(imported, true);
      dirtyRef.current = false;
      setDraftDirty(false);
      setStatus((current) => ({ ...current, profile: normalized }));
      setSection("overview");
      resetProfileScopedState();
      await refreshProfiles();
      await refreshStatus();
      notify(`Servidor ${normalized.name} encontrado`);
    } catch (error) {
      notify(`Não foi possível importar o servidor: ${String(error)}`);
    } finally {
      setBusy(null);
    }
  };

  const sendCommand = async () => {
    const cleanCommand = command.trim();
    if (!cleanCommand || !status.running) return;
    setBusy("command");
    try {
      await serverApi.sendCommand(cleanCommand);
      setCommand("");
      window.setTimeout(() => void refreshLog(), 180);
    } catch (error) {
      notify(`Falha ao enviar comando: ${String(error)}`);
    } finally {
      setBusy(null);
    }
  };

  const clearLog = async () => {
    setBusy("clear");
    try {
      await serverApi.clearLog();
      setLog("O console está vazio.");
    } catch (error) {
      notify(String(error));
    } finally {
      setBusy(null);
    }
  };

  const togglePlayit = async () => {
    setBusy("playit");
    try {
      const nextStatus = playit.service_running ? await serverApi.stopPlayit() : await serverApi.startPlayit();
      setPlayit(nextStatus);
      notify(nextStatus.running ? "Playit conectado" : nextStatus.service_running ? "Serviço do Playit iniciado" : "Playit parado");
    } catch (error) {
      notify(`Falha no playit: ${String(error)}`);
      await refreshPlayit();
    } finally {
      setBusy(null);
    }
  };

  const setupPlayit = async () => {
    setBusy("playit");
    try {
      const url = await serverApi.setupPlayit();
      await serverApi.openUrl(url);
      notify("Conclua a vinculação na página do playit.gg que foi aberta");
      window.setTimeout(() => void refreshPlayit(), 1200);
    } catch (error) {
      notify(`Não foi possível vincular o Playit: ${String(error)}`);
    } finally {
      setBusy(null);
    }
  };

  const repairPlayitAccess = async () => {
    setBusy("playit");
    try {
      const next = await serverApi.repairPlayitAccess();
      setPlayit(next);
      notify("Acesso ao serviço Playit liberado para o VEX");
    } catch (error) {
      notify(String(error));
    } finally {
      setBusy(null);
    }
  };

  const exportClientPack = async () => {
    setBusy("export");
    setExportResult(null);
    try {
      const result = await serverApi.exportClientPack(includeUnknown, includeConfigs);
      setExportResult(result);
      notify(`Pack criado com ${result.included_files} arquivos`);
    } catch (error) {
      notify(`Falha ao exportar pack: ${String(error)}`);
    } finally {
      setBusy(null);
    }
  };

  const createPlayableInstance = async () => {
    const confirmed = window.confirm(`Criar uma instância jogável para “${savedProfile.name}”?\n\nEla ficará no HD, aparecerá na Biblioteca do VEX e receberá os mods de cliente selecionados. Plugins, mundos e credenciais não serão copiados.`);
    if (!confirmed) return;
    setBusy("instance");
    setPlayableInstanceResult(null);
    try {
      const result = await serverApi.createPlayableInstance(includeUnknown, includeConfigs);
      setPlayableInstanceResult(result);
      notify(`Instância ${result.instance.name} criada`);
    } catch (error) {
      notify(`Falha ao criar instância: ${String(error)}`);
    } finally {
      setBusy(null);
    }
  };

  const toggleContent = async (item: ServerContentItem) => {
    if (status.running) {
      notify("Pare o servidor antes de ativar ou desativar conteúdo.");
      return;
    }
    const action = item.enabled ? "desativar" : "ativar";
    if (!window.confirm(`${action[0].toUpperCase()}${action.slice(1)} ${item.name}?\n\nO VEX apenas renomeará o arquivo; nada será apagado.`)) return;
    setBusy("content-toggle");
    try {
      const items = await serverApi.setContentEnabled(item.path, !item.enabled);
      setContentItems(items);
      setExportResult(null);
      await refreshMapStatus();
      notify(`${item.name} foi ${item.enabled ? "desativado" : "ativado"}.`);
    } catch (error) {
      notify(`Não foi possível ${action} o conteúdo: ${String(error)}`);
    } finally {
      setBusy(null);
    }
  };

  const previewChunks = async () => {
    setBusy("map");
    setMapMessage("");
    try {
      const preview = await serverApi.previewChunks(mapWorld, mapShape, mapX, mapZ, mapRadius);
      setChunkPreview(preview);
      setMapMessage(preview.message);
    } catch (error) {
      setChunkPreview(null);
      setMapMessage(String(error));
    } finally {
      setBusy(null);
    }
  };

  const startChunks = async () => {
    if (!chunkPreview) {
      notify("Calcule a estimativa antes de iniciar a geração.");
      return;
    }
    if (!chunkPreview.safe) {
      notify(chunkPreview.message);
      return;
    }
    const confirmed = window.confirm(`Gerar aproximadamente ${chunkPreview.chunks.toLocaleString("pt-BR")} chunks a partir de X ${mapX}, Z ${mapZ}?\n\nEstimativa conservadora: ${chunkPreview.estimated_disk_gb.toFixed(1)} GB.\nComando: ${chunkPreview.command}`);
    if (!confirmed) return;
    setBusy("map");
    try {
      const command = await serverApi.startChunks(mapWorld, mapShape, mapX, mapZ, mapRadius);
      setMapMessage(`Comando enviado: ${command}`);
      window.setTimeout(() => void refreshMapTask(), 400);
      notify("Geração de chunks iniciada");
    } catch (error) {
      setMapMessage(String(error));
      notify(`Geração bloqueada: ${String(error)}`);
    } finally {
      setBusy(null);
    }
  };

  const controlChunks = async (action: "pause" | "continue" | "cancel" | "progress") => {
    if (action === "cancel" && !window.confirm("Cancelar a tarefa do Chunky? Os chunks já gerados não serão apagados.")) return;
    setBusy("map");
    try {
      const command = await serverApi.controlChunks(action, mapWorld || undefined);
      setMapMessage(`Comando enviado: ${command}. Veja o progresso no Console.`);
      if (action === "progress") setSection("console");
    } catch (error) {
      setMapMessage(String(error));
      notify(String(error));
    } finally {
      setBusy(null);
    }
  };

  const renderMapRegion = async () => {
    if (!mapStatus.bluemap_enabled) {
      notify("Ative o BlueMap antes de renderizar.");
      return;
    }
    const radiusBlocks = Math.max(1, Math.floor(renderRadiusChunks)) * 16;
    if (!window.confirm(`Renderizar o BlueMap ao redor de X ${mapX}, Z ${mapZ}?\n\nÁrea: ${renderRadiusChunks} chunks de raio (${radiusBlocks} blocos) usando ${blueMapThreads} thread(s). O VEX pausará a renderização quando terminar.`)) return;
    setBusy("map");
    try {
      const command = await serverApi.renderBlueMap(mapWorld, mapX, mapZ, radiusBlocks, blueMapThreads, forceBlueMapRender);
      setMapFocus({ world: mapWorld, x: Math.floor(mapX), z: Math.floor(mapZ) });
      setMapMessage(`Renderização solicitada: ${command}`);
      window.setTimeout(() => void refreshMapTask(), 400);
      notify("Renderização do mapa solicitada");
    } catch (error) {
      setMapMessage(String(error));
      notify(String(error));
    } finally {
      setBusy(null);
    }
  };

  const installRecommendedMapStack = async () => {
    if (status.running) {
      notify("Pare o servidor antes de instalar plugins ou mods do mapa.");
      return;
    }
    const confirmed = window.confirm("Instalar o Chunky neste servidor?\n\nO VEX buscará a versão exata do Chunky e, quando necessário, Fabric API diretamente do CDN oficial do Modrinth. Os hashes SHA-512 serão verificados.");
    if (!confirmed) return;
    setBusy("map-install");
    try {
      const result = await serverApi.installMapStack("chunky", false);
      setMapInstallResult(result);
      setMapInstallResultProvider("chunky");
      await Promise.all([refreshContent(), refreshMapStatus()]);
      notify(result.installed.length ? "Chunky instalado com verificação de integridade." : "O Chunky já estava instalado.");
    } catch (error) {
      setMapMessage(String(error));
      notify(`Não foi possível instalar a stack do mapa: ${String(error)}`);
    } finally {
      setBusy(null);
    }
  };

  const installBlueMap = async () => {
    if (status.running) {
      notify("Pare o servidor antes de instalar e configurar o BlueMap.");
      return;
    }
    const confirmed = window.confirm("Instalar e configurar o BlueMap 3D?\n\nAo continuar, você confirma que possui uma licença do Minecraft Java, aceita a EULA da Mojang e autoriza o BlueMap a baixar o cliente oficial necessário para texturizar os modelos 3D. Os JARs virão do Modrinth com SHA-512 verificado; modelos e tiles ficarão no HD selecionado.");
    if (!confirmed) return;
    setBusy("map-install");
    try {
      const result = await serverApi.installMapStack("bluemap", true);
      setMapInstallResult(result);
      setMapInstallResultProvider("bluemap");
      await Promise.all([refreshContent(), refreshMapStatus()]);
      notify(result.installed.length ? "BlueMap 3D instalado e configurado." : "BlueMap já estava instalado; configuração conferida.");
    } catch (error) {
      notify(`Não foi possível instalar o BlueMap: ${String(error)}`);
    } finally {
      setBusy(null);
    }
  };

  const focusMapPlayer = (player: Awaited<ReturnType<typeof serverApi.getLastPlayerLocation>>) => {
    if (typeof player.x !== "number" || typeof player.z !== "number") {
      notify(`${player.name} ainda não possui coordenadas salvas.`);
      return;
    }
    const x = Math.floor(player.x);
    const z = Math.floor(player.z);
    setMapX(x);
    setMapZ(z);
    setMapFocus({ world: player.world, x, z });
    notify(`Centro movido para ${player.name}: X ${x}, Z ${z}`);
  };

  const useLastPlayerLocation = async () => {
    setBusy("map");
    try {
      const player = await serverApi.getLastPlayerLocation();
      focusMapPlayer(player);
      setMapWorld(serverWorldFromPlayerWorld(player.world, mapStatus.world_name));
      setMapMessage(`Usando a última posição salva de ${player.name}: X ${Math.floor(player.x ?? 0)}, Z ${Math.floor(player.z ?? 0)}.`);
    } catch (error) {
      notify(String(error));
    } finally {
      setBusy(null);
    }
  };

  const saveBlueMapArea = async () => {
    if (status.running) {
      notify("Pare este servidor para salvar os limites permanentes do BlueMap. A renderização avulsa pode ser feita com ele ligado.");
      return;
    }
    setBusy("map");
    try {
      const result = await serverApi.configureBlueMapArea(mapWorld, Math.floor(mapX), Math.floor(mapZ), Math.max(1, Math.floor(renderRadiusChunks)));
      setMapMessage(`Área 3D salva: ${result.radius_chunks} chunks ao redor de X ${result.x}, Z ${result.z}.`);
      notify("Área inicial do BlueMap configurada");
    } catch (error) {
      setMapMessage(String(error));
      notify(String(error));
    } finally {
      setBusy(null);
    }
  };

  const toggleBlueMap = async () => {
    const enable = mapStatus.bluemap_pending_enabled !== undefined ? !mapStatus.bluemap_pending_enabled : !mapStatus.bluemap_enabled;
    setBusy("map");
    try {
      const next = await serverApi.setBlueMapEnabled(enable);
      setMapStatus(next);
      setBluemapLoaded(false);
      notify(status.running ? `BlueMap será ${enable ? "ativado" : "desativado"} no próximo reinício.` : `BlueMap ${enable ? "ativado" : "desativado"}.`);
    } catch (error) {
      notify(String(error));
    } finally {
      setBusy(null);
    }
  };

  const controlBlueMap = async (action: "status" | "pause" | "resume" | "tasks" | "cancel" | "reload" | "freeze" | "unfreeze" | "troubleshoot") => {
    if (action === "cancel" && !window.confirm("Cancelar todas as tarefas atuais do BlueMap? Os arquivos já renderizados serão mantidos.")) return;
    setBusy("map");
    try {
      const command = await serverApi.controlBlueMap(action, mapWorld, Math.floor(mapX), Math.floor(mapZ));
      setMapMessage(`Comando executado: ${command}`);
      window.setTimeout(() => void refreshMapTask(), 300);
    } catch (error) {
      notify(String(error));
    } finally {
      setBusy(null);
    }
  };

  const openServerFolder = () => void serverApi.openPath(savedProfile.directory).catch((error) => notify(String(error)));
  const metrics = { ...emptyServerMetrics, ...(status.metrics ?? {}) };
  const systemMemoryPercent = percentage(metrics.system_memory_used_mb, metrics.system_memory_total_mb);
  const allocatedMemoryMb = Math.max(1, savedProfile.memory_gb) * 1024;
  const serverMemoryPercent = percentage(metrics.memory_mb, allocatedMemoryMb);
  const serverBusy = busy === "server";
  const modCount = contentItems.filter((item) => item.kind === "mod").length;
  const pluginCount = contentItems.filter((item) => item.kind === "plugin").length;
  const unknownModCount = contentItems.filter((item) => item.kind === "mod" && item.side === "unknown").length;
  const knownSoftwareOptions = ["vanilla", "paper", "fabric"];
  const importedProfile = !draftProfile.managed;
  const importedScript = importedProfile && draftProfile.launch_mode === "script";
  const mapInstallSupported = ["paper", "fabric", "forge", "neoforge", "modded"].includes(savedProfile.software.toLowerCase());
  const mapStackReady = mapStatus.chunky_installed;
  const activeMapUrl = mapStatus.bluemap_web_url;
  const activeMapOnline = mapStatus.bluemap_online;
  const activeRenderStorageSafe = mapStatus.bluemap_storage_safe;
  const workspaceStorageSafe = mapWorkspace === "worldgen" ? mapStatus.world_generation_safe : activeRenderStorageSafe;
  const storagePanelMessage = mapWorkspace === "worldgen" ? mapStatus.world_generation_message : mapStatus.bluemap_storage_message;
  const embeddedMapUrl = activeMapUrl ? focusedMapUrl(activeMapUrl, mapFocus) : "";

  return <div className="server-page">
    {!libraryLoaded ? <section className="section-block server-library-empty"><span className="server-library-empty-icon"><Server size={34} /></span><span className="overline">Biblioteca de servidores</span><h1>Carregando seus servidores...</h1></section> : profiles.length === 0 ? <section className="section-block server-library-empty"><span className="server-library-empty-icon"><Server size={34} /></span><span className="overline">Primeiro servidor</span><h1>Nenhum servidor adicionado</h1><p>Comece do zero com uma configuração guiada ou conecte uma pasta que já existe. O VEX só baixa e altera arquivos depois da sua confirmação.</p><div className="server-library-empty-actions"><button className="primary-button" disabled={busy !== null} onClick={openCreateServerDialog}><Plus size={17} />Criar novo servidor</button><button className="secondary-button" disabled={busy !== null} onClick={() => void importServer()}><FolderOpen size={17} />{busy === "import" ? "Analisando..." : "Encontrar servidor existente"}</button></div><div className="server-library-empty-notes"><span><ShieldCheck size={16} /><b>Importação segura</b><small>Não reescreve seu start.sh, mundo, mods ou server.properties.</small></span><span><HardDrive size={16} /><b>Dados novos no HD</b><small>Servidores criados pelo VEX usam a pasta de armazenamento configurada.</small></span></div>{connectionError && <div className="server-alert error" role="alert"><WifiOff size={17} /><span><b>O gerenciador não respondeu</b><small>{connectionError}</small></span><button onClick={() => void refreshProfiles()}>Tentar novamente</button></div>}</section> : <>
    <div className="page-intro server-page-intro">
      <div><span className="overline">Controle local</span><h1>{savedProfile.name}</h1><p>{softwareLabel(savedProfile.software)} · Minecraft {savedProfile.version} · {savedProfile.managed ? "Gerenciado pelo VEX" : "Servidor encontrado"}</p></div>
      <div className="server-header-actions">
        <button className="secondary-button" disabled={busy !== null || draftDirty} title={draftDirty ? "Salve ou descarte as alterações primeiro" : undefined} onClick={() => void importServer()}><FolderOpen size={17} />{busy === "import" ? "Analisando..." : "Encontrar servidor"}</button>
        <button disabled={busy !== null || draftDirty} title={draftDirty ? "Salve ou descarte as alterações primeiro" : undefined} className={status.running ? "danger-button" : "primary-button"} onClick={() => void toggleServer()}>{status.running ? <Square size={16} fill="currentColor" /> : <Play size={17} fill="currentColor" />}{serverBusy ? "Aguarde..." : status.running ? "Parar servidor" : "Iniciar servidor"}</button>
      </div>
    </div>

    <section className="server-library-bar" aria-label="Biblioteca de servidores">
      <label className="server-profile-picker">
        <Server size={18} />
        <span><small>Servidor selecionado</small><select aria-label="Selecionar servidor" value={savedProfile.id} disabled={busy !== null || draftDirty} onChange={(event) => void switchProfile(event.target.value)}>{profiles.map((profile) => <option key={profile.id} value={profile.id}>{runningProfileIds.has(profile.id) ? "● " : ""}{profile.name} · {softwareLabel(profile.software)} {profile.version}{runningProfileIds.has(profile.id) ? " · online" : ""}</option>)}</select></span>
      </label>
      <span className="server-profile-count">{profiles.length} {profiles.length === 1 ? "servidor" : "servidores"}{runningProfileIds.size ? ` · ${runningProfileIds.size} online` : ""}</span>
      <button className="secondary-button compact" disabled={busy !== null || draftDirty} onClick={openCreateServerDialog}><Plus size={15} />Novo servidor</button>
      <button className="secondary-button compact danger-text" disabled={busy !== null || status.running || draftDirty} title="Remove apenas o perfil; nunca apaga a pasta" onClick={() => void forgetCurrentServer()}><Trash2 size={15} />Esquecer</button>
    </section>

    {connectionError && <div className="server-alert error" role="alert"><WifiOff size={17} /><span><b>O gerenciador não respondeu</b><small>{connectionError}</small></span><button onClick={() => void refreshStatus()}><RefreshCw size={15} />Tentar novamente</button></div>}
    {draftDirty && <div className="server-alert pending"><Settings size={17} /><span><b>Alterações ainda não salvas</b><small>O monitoramento continua ativo sem substituir este rascunho.</small></span><button onClick={discardDraft}>Descartar</button><button onClick={() => setSection("settings")}>Revisar</button></div>}

    <div className="server-status-band" aria-live="polite">
      <span className={`server-power ${status.running ? "online" : ""}`}><Zap size={21} /></span>
      <div className="grow"><b>{status.running ? status.recovered ? "Servidor online · controle recuperado" : "Servidor online" : "Servidor parado"}</b><span>{status.running ? `localhost:${savedProfile.port}${status.pid ? ` · PID ${status.pid}` : ""} · ${status.detail}` : status.detail || "Pronto para iniciar pelo VEX."}</span></div>
      <div className="server-stat"><span>Versão</span><b>{savedProfile.version} {softwareLabel(savedProfile.software)}</b></div>
      <div className="server-stat"><span>Memória</span><b>{savedProfile.memory_gb} GB alocados</b></div>
      <div className="server-stat"><span>Playit</span><b>{playit.running ? "Conectado" : playit.needs_setup ? "Vincular conta" : playit.service_running ? "Agente ativo" : playit.installed ? "Parado" : "Não encontrado"}</b></div>
    </div>
    {status.running && !status.ready && <div className="server-startup-progress" role="progressbar" aria-valuemin={0} aria-valuemax={100} aria-valuenow={status.startup_percent}><span><b>{status.startup_label}</b><small>{status.startup_percent}%</small></span><div><i style={{ width: `${status.startup_percent}%` }} /></div></div>}

    <nav className="server-tabs" aria-label="Áreas do servidor" role="tablist">
      {sections.map((item) => <button key={item.id} role="tab" aria-selected={section === item.id} className={section === item.id ? "active" : ""} onClick={() => setSection(item.id)}>{item.icon}<span>{item.label}</span></button>)}
    </nav>

    {section === "overview" && <div className="server-overview-grid">
      <section className="section-block server-overview-card">
        <div className="section-heading compact"><div><span className="overline">Servidor atual</span><h2>Resumo</h2></div><IconButton label="Abrir pasta do servidor" onClick={openServerFolder}><FolderOpen size={17} /></IconButton></div>
        <div className="server-identity">
          <span className={`server-identity-icon ${status.running ? "online" : ""}`}><Server size={27} /></span>
          <span><b>{savedProfile.name}</b><small>{savedProfile.directory}</small></span>
        </div>
        <div className="config-line"><span>Inicialização</span><b>{savedProfile.managed ? "Automática pelo VEX" : savedProfile.start_target || savedProfile.launch_mode}</b></div>
        <div className="config-line"><span>Endereço local</span><b>localhost:{savedProfile.port}</b></div>
        <div className="config-line"><span>Jogadores</span><b>Até {savedProfile.max_players}</b></div>
        <div className="config-line"><span>Conta original</span><b>{savedProfile.online_mode ? "Obrigatória" : "Opcional"}</b></div>
        <div className="server-card-actions"><button className="secondary-button compact" onClick={openServerFolder}><FolderOpen size={15} />Abrir pasta</button><button className="secondary-button compact" onClick={() => setSection("settings")}><Settings size={15} />Configurar</button></div>
      </section>

      <section className="section-block server-metrics-panel">
        <div className="section-heading compact"><div><span className="overline">Leitura do sistema</span><h2>Desempenho em tempo real</h2></div><span className={status.running ? "online-label" : "offline-label"}>{status.running ? "Monitorando" : "Em espera"}</span></div>
        <div className="server-metrics-grid">
          <MetricCard icon={<Cpu size={18} />} label="CPU do servidor" value={`${metrics.cpu_percent.toFixed(1)}%`} note={status.running ? "Processo Java" : "Servidor parado"} progress={metrics.cpu_percent} />
          <MetricCard icon={<HardDrive size={18} />} label="RAM do servidor" value={formatMemory(metrics.memory_mb)} note={`de ${savedProfile.memory_gb} GB alocados`} progress={serverMemoryPercent} />
          <MetricCard icon={<Activity size={18} />} label="RAM do sistema" value={formatMemory(metrics.system_memory_used_mb)} note={`de ${formatMemory(metrics.system_memory_total_mb)}`} progress={systemMemoryPercent} />
          <MetricCard icon={<Box size={18} />} label="Processos" value={String(metrics.process_count)} note="Árvore monitorada" />
        </div>
      </section>

      <section className="section-block server-overview-card server-network-summary">
        <div className="section-heading compact"><div><span className="overline">Acesso externo</span><h2>Playit</h2></div>{playit.running ? <Wifi size={18} /> : <WifiOff size={18} />}</div>
        <div className={`server-service-state ${playit.running ? "online" : ""}`}><span /><b>{playit.running ? "Túnel conectado" : playit.needs_setup ? "Conta ainda não vinculada" : playit.service_running ? "Agente iniciado" : playit.installed ? "Instalado e parado" : "Playit não encontrado"}</b></div>
        <p>{playit.detail}</p>
        <div className="server-card-actions"><button className="secondary-button compact" onClick={() => setSection("network")}><Globe2 size={15} />Abrir rede</button></div>
      </section>
    </div>}

    {section === "console" && <section className="section-block console-block server-console-panel">
      <div className="section-heading compact"><div><span className="overline">Saída em tempo real</span><h2>Console do servidor</h2></div><div className="console-actions"><button className={`secondary-button compact ${followingLog ? "following" : ""}`} onClick={() => setFollowingLog(!followingLog)}><span className="status-dot" />{followingLog ? "Acompanhando" : "Pausado"}</button><IconButton label="Atualizar console" onClick={() => void refreshLog()}><RefreshCw size={16} /></IconButton><IconButton label="Limpar console" disabled={busy !== null || status.running} onClick={() => void clearLog()}><X size={16} /></IconButton></div></div>
      <pre ref={consoleRef} className="console-output server-log">{log}</pre>
      <div className="console-input"><span>&gt;</span><input value={command} onChange={(event) => setCommand(event.target.value)} onKeyDown={(event) => { if (event.key === "Enter") void sendCommand(); }} placeholder={status.running ? "Digite um comando..." : "Inicie o servidor para usar o console"} disabled={!status.running || busy === "command"} /><button disabled={!status.running || busy === "command" || !command.trim()} onClick={() => void sendCommand()}>{busy === "command" ? "Enviando..." : "Enviar"}</button></div>
    </section>}

    {section === "content" && <div className="server-content-grid">
      <section className="section-block server-content-panel">
        <div className="section-heading compact"><div><span className="overline">Arquivos detectados</span><h2>Mods e plugins</h2></div><div className="console-actions"><span className="counter">{modCount} mods · {pluginCount} plugins</span><IconButton label="Atualizar conteúdo" disabled={busy === "content"} onClick={() => void refreshContent()}><RefreshCw size={16} /></IconButton></div></div>
        {contentError && <div className="server-inline-warning error"><AlertTriangle size={17} /><span><b>Falha ao analisar conteúdo</b><small>{contentError}</small></span></div>}
        {!contentError && !contentItems.length && <div className="empty-state large-empty"><PackagePlus size={28} /><span>Nenhum mod ou plugin JAR foi encontrado nas pastas do servidor.</span><button className="secondary-button compact" onClick={openServerFolder}><FolderOpen size={15} />Abrir pasta</button></div>}
        {!!contentItems.length && <div className="server-content-list">
          {contentItems.map((item) => <article className={`server-content-row ${item.enabled ? "" : "disabled"}`} key={item.path}>
            <span className="server-content-icon">{item.kind === "plugin" ? <Archive size={19} /> : <PackagePlus size={19} />}</span>
            <span className="grow"><b>{item.name}</b><small>{item.kind === "plugin" ? "Plugin" : "Mod"} · {item.size_mb.toFixed(1)} MB</small></span>
            <span className={`server-side-tag side-${item.side}`}>{contentSideLabel(item.side)}</span>
            {!item.enabled && <span className="offline-label">Desativado</span>}
            <IconButton label={item.enabled ? `Desativar ${item.name}` : `Ativar ${item.name}`} disabled={busy !== null || status.running} onClick={() => void toggleContent(item)}>{item.enabled ? <PowerOff size={15} /> : <Power size={15} />}</IconButton>
          </article>)}
        </div>}
      </section>

      <aside className="section-block server-export-panel">
        <div className="section-heading compact"><div><span className="overline">Cliente compatível</span><h2>Jogar neste servidor</h2></div><FileArchive size={19} /></div>
        <p>Crie uma instância pronta no VEX ou exporte um ZIP para amigos. Plugins, mundos, tokens e mods declarados somente para servidor ficam de fora.</p>
        <label className="server-switch-row"><span><b>Incluir mods desconhecidos</b><small>{unknownModCount ? `${unknownModCount} arquivo(s) sem metadado de lado` : "Todos os mods têm lado conhecido"}</small></span><input type="checkbox" checked={includeUnknown} onChange={(event) => setIncludeUnknown(event.target.checked)} /></label>
        <label className="server-switch-row"><span><b>Incluir configurações seguras</b><small>Exclui segredos, credenciais, links e arquivos grandes.</small></span><input type="checkbox" checked={includeConfigs} onChange={(event) => setIncludeConfigs(event.target.checked)} /></label>
        <div className="server-inline-warning"><ShieldCheck size={17} /><span><b>Separação segura</b><small>A instância e o ZIP usam as mesmas regras para excluir conteúdo exclusivo do servidor.</small></span></div>
        <div className="server-client-actions"><button className="primary-button" disabled={busy !== null} onClick={() => void createPlayableInstance()}><Box size={17} />{busy === "instance" ? "Criando instância..." : "Criar instância jogável"}</button><button className="secondary-button" disabled={busy !== null || !modCount} onClick={() => void exportClientPack()}><FileArchive size={17} />{busy === "export" ? "Criando ZIP..." : "Criar pack para amigos"}</button></div>
        {playableInstanceResult && <div className="server-export-result"><CheckCircle2 size={18} /><span><b>Instância pronta</b><small>{playableInstanceResult.included_files} incluídos · {playableInstanceResult.skipped_files} ignorados · {playableInstanceResult.server_address}</small><code>{playableInstanceResult.instance.profile_dir}</code></span><button className="primary-button compact" onClick={onOpenLibrary}><Play size={15} />Abrir na Biblioteca</button>{playableInstanceResult.warnings.map((warning) => <p key={warning}>{warning}</p>)}</div>}
        {exportResult && <div className="server-export-result"><CheckCircle2 size={18} /><span><b>Pack criado</b><small>{exportResult.included_files} incluídos · {exportResult.skipped_files} ignorados</small><code>{exportResult.path}</code></span><button className="secondary-button compact" onClick={() => void serverApi.openPath(exportResult.path).catch((error) => notify(String(error)))}><FolderOpen size={15} />Mostrar</button>{exportResult.warnings.map((warning) => <p key={warning}>{warning}</p>)}</div>}
        <div className="server-recommendations"><span className="overline">Recomendações compatíveis</span><h3>{savedProfile.software === "paper" ? "Plugins para Paper" : savedProfile.software === "fabric" ? "Mods para Fabric" : "Conteúdo para o servidor"}</h3><p>Abra a descoberta do VEX para procurar projetos. A instalação só oferece este servidor quando versão, tipo e loader forem compatíveis.</p><button className="secondary-button" onClick={onOpenDiscover}><PackagePlus size={16} />Descobrir conteúdo</button></div>
      </aside>
    </div>}

    {section === "map" && <>
      <nav className="server-map-workspaces" aria-label="Ferramentas do mapa">
        <button className={mapWorkspace === "visualization" ? "active" : ""} onClick={() => setMapWorkspace("visualization")}><Map size={16} /><span><b>BlueMap 3D</b><small>Visualizar e renderizar</small></span></button>
        <button className={mapWorkspace === "worldgen" ? "active" : ""} onClick={() => setMapWorkspace("worldgen")}><Layers3 size={16} /><span><b>Gerar mundo</b><small>Chunks com o Chunky</small></span></button>
      </nav>
      <div className={`server-map-layout ${mapWorkspace === "worldgen" ? "worldgen" : "visualization"}`}>
        {mapWorkspace === "visualization" && <section className={`section-block server-map-viewer ${mapExpanded ? "expanded" : ""}`}>
          <div className="section-heading compact"><div><span className="overline">Mapa do mundo</span><h2>Visualização BlueMap 3D</h2></div><div className="console-actions"><span className={activeMapOnline ? "online-label" : "offline-label"}>{activeMapOnline ? "Mapa online" : mapStatus.bluemap_enabled ? "Aguardando servidor" : "BlueMap desativado"}</span><IconButton label={mapExpanded ? "Sair da tela ampliada" : "Ampliar mapa"} onClick={() => setMapExpanded((current) => !current)}>{mapExpanded ? <Minimize2 size={16} /> : <Maximize2 size={16} />}</IconButton><IconButton label="Atualizar mapa" onClick={() => void refreshMapStatus()}><RefreshCw size={16} /></IconButton>{activeMapUrl && <IconButton label="Abrir no navegador" onClick={() => void serverApi.openUrl(activeMapUrl).catch((error) => notify(String(error)))}><ExternalLink size={16} /></IconButton>}</div></div>
          {activeMapOnline && activeMapUrl && !bluemapLoaded ? <div className="server-map-empty"><Layers3 size={42} /><h3>BlueMap pronto para carregar</h3><p>A visualização só usa GPU e memória enquanto estiver aberta. A renderização do servidor continua separada e fica pausada quando não há uma tarefa pedida pelo VEX.</p><button className="primary-button" onClick={() => setBluemapLoaded(true)}><Layers3 size={16} />Carregar visualização 3D</button></div>
            : activeMapOnline && activeMapUrl ? <iframe className="server-map-frame" title="BlueMap 3D" src={embeddedMapUrl} sandbox="allow-scripts allow-same-origin allow-forms allow-popups allow-pointer-lock" />
            : <div className="server-map-empty"><Map size={42} /><h3>{mapStatus.bluemap_installed ? mapStatus.bluemap_enabled ? "BlueMap aguardando o servidor" : "BlueMap está desativado" : "Instale o BlueMap 3D"}</h3><p>{mapStatus.bluemap_installed ? "Inicie o servidor para abrir o mapa, ou ative o BlueMap se quiser carregá-lo no próximo início." : "O BlueMap cria uma visualização 3D navegável e mantém os dados pesados na pasta padrão do VEX no HD."}</p></div>}
        </section>}

        <aside className="server-map-controls">
          {mapWorkspace === "visualization" ? <>
            <section className="section-block server-map-install server-bluemap-install">
              <div className="section-heading compact"><div><span className="overline">Visualização 3D</span><h2>BlueMap</h2></div>{mapStatus.bluemap_configured ? <CheckCircle2 size={19} /> : <Download size={19} />}</div>
              <div className={`server-inline-warning ${mapStatus.bluemap_configured ? "success" : ""}`}><ShieldCheck size={17} /><span><b>{mapStatus.bluemap_configured ? mapStatus.bluemap_enabled ? "Instalado e ativado" : "Instalado e desativado" : "Instalação oficial assistida"}</b><small>Quando não está renderizando, o custo de CPU do BlueMap cai; o servidor web apenas entrega os arquivos já prontos.</small></span></div>
              {!mapStatus.bluemap_storage_safe && <div className="server-inline-warning error"><HardDrive size={17} /><span><b>Sem espaço seguro</b><small>{mapStatus.bluemap_storage_message}</small></span></div>}
              {!mapStatus.bluemap_configured && <button className="primary-button" disabled={busy !== null || status.running || !mapInstallSupported || !mapStatus.bluemap_storage_safe} onClick={() => void installBlueMap()}><Download size={16} />{busy === "map-install" ? "Baixando e configurando..." : mapStatus.bluemap_installed ? "Finalizar configuração" : "Instalar BlueMap 3D"}</button>}
              {mapStatus.bluemap_installed && <button className={mapStatus.bluemap_enabled ? "danger-button" : "primary-button"} disabled={busy !== null} onClick={() => void toggleBlueMap()}>{mapStatus.bluemap_enabled ? <PowerOff size={16} /> : <Power size={16} />}{mapStatus.bluemap_pending_enabled !== undefined ? `Agendado: ${mapStatus.bluemap_pending_enabled ? "ativar" : "desativar"} · clique para inverter` : mapStatus.bluemap_enabled ? status.running ? "Desativar no próximo início" : "Desativar BlueMap" : status.running ? "Ativar no próximo início" : "Ativar BlueMap"}</button>}
              <small className="server-license-note">BlueMap: MIT · Fabric API: Apache-2.0. Desativar renomeia somente o JAR; mapa e configurações são preservados.</small>
              {mapInstallResult && mapInstallResultProvider === "bluemap" && <div className="server-map-install-result"><b>{mapInstallResult.source}</b>{mapInstallResult.installed.map((item) => <span key={item.destination}><Check size={14} /><em>{item.project} {item.version}</em><small>{item.filename} · {item.license}</small></span>)}</div>}
            </section>

            <section className="section-block server-map-task">
              <div className="section-heading compact"><div><span className="overline">Renderização sob demanda</span><h2>Área do BlueMap</h2></div><Layers3 size={19} /></div>
              <div className="server-map-form">
                <label>Mapa<input value={mapWorld} onChange={(event) => setMapWorld(event.target.value)} /></label>
                <label>Threads<select value={blueMapThreads} onChange={(event) => setBlueMapThreads(Number(event.target.value))}>{Array.from({ length: Math.max(1, mapStatus.bluemap_max_threads) }, (_, index) => index + 1).map((value) => <option key={value} value={value}>{value}</option>)}</select></label>
                <label>Coordenada X<input type="number" value={mapX} onChange={(event) => setMapX(Number(event.target.value))} /></label>
                <label>Coordenada Z<input type="number" value={mapZ} onChange={(event) => setMapZ(Number(event.target.value))} /></label>
                <label className="server-map-radius">Raio em chunks<input type="number" min="1" max="3125" value={renderRadiusChunks} onChange={(event) => setRenderRadiusChunks(Number(event.target.value))} /><small>{Math.max(1, Math.floor(renderRadiusChunks || 1)) * 16} blocos de raio</small></label>
              </div>
              <button className="secondary-button server-player-location-button" disabled={busy !== null} onClick={() => void useLastPlayerLocation()}><LocateFixed size={16} />Usar última posição salva do jogador</button>
              <label className="server-check-row"><input type="checkbox" checked={forceBlueMapRender} onChange={(event) => setForceBlueMapRender(event.target.checked)} /><span><b>Forçar nova renderização</b><small>Use quando os blocos mudaram e o BlueMap não atualizou a área.</small></span></label>
              <div className="server-inline-warning success"><ShieldCheck size={17} /><span><b>Processa e para</b><small>O VEX cancela filas antigas, renderiza apenas esta área, mostra progresso real e envia pausa quando termina.</small></span></div>
              <div className="server-map-actions"><button className="primary-button" disabled={busy !== null || !mapStatus.bluemap_enabled || !activeRenderStorageSafe || !status.running} onClick={() => void renderMapRegion()}><Map size={16} />Renderizar área 3D</button><button className="secondary-button" disabled={busy !== null || status.running || !mapStatus.bluemap_configured} onClick={() => void saveBlueMapArea()}><Save size={16} />Fixar centro inicial</button></div>
              <div className={`server-map-progress ${mapTask.active ? "active" : ""}`}><span><b>{mapTask.label}</b><small>{mapTask.determinate && typeof mapTask.percent === "number" ? `${mapTask.percent.toFixed(1)}%` : mapTask.active ? "Aguardando medição" : "Em espera"}</small></span><div><i style={{ width: `${mapTask.percent ?? 0}%` }} /></div><p>{mapTask.detail}</p>{mapTask.eta_seconds !== undefined && <em>Estimativa restante: {mapTask.eta_seconds >= 3600 ? `${(mapTask.eta_seconds / 3600).toFixed(1)} h` : `${Math.max(1, Math.ceil(mapTask.eta_seconds / 60))} min`}</em>}</div>
              {mapMessage && <div className="server-map-message">{mapMessage}</div>}
            </section>

            <section className="section-block server-map-task">
              <div className="section-heading compact"><div><span className="overline">Comandos sem terminal</span><h2>Controle BlueMap</h2></div><TerminalSquare size={18} /></div>
              <div className="server-bluemap-commands"><button onClick={() => void controlBlueMap("status")} disabled={busy !== null || !status.running}>Ver status</button><button onClick={() => void controlBlueMap("tasks")} disabled={busy !== null || !status.running}>Ver tarefas</button><button onClick={() => void controlBlueMap("pause")} disabled={busy !== null || !status.running}>Pausar</button><button onClick={() => void controlBlueMap("resume")} disabled={busy !== null || !status.running}>Retomar</button><button onClick={() => void controlBlueMap("reload")} disabled={busy !== null || !status.running}>Recarregar</button><button onClick={() => void controlBlueMap("freeze")} disabled={busy !== null || !status.running}>Congelar mapa</button><button onClick={() => void controlBlueMap("unfreeze")} disabled={busy !== null || !status.running}>Descongelar</button><button onClick={() => void controlBlueMap("troubleshoot")} disabled={busy !== null || !status.running}>Diagnosticar ponto</button><button className="danger" onClick={() => void controlBlueMap("cancel")} disabled={busy !== null || !status.running}>Cancelar tarefas</button></div>
              <small className="server-license-note">Pausar persiste após reiniciar. “Retomar” permite que o BlueMap processe a fila; o VEX volta a pausar ao concluir uma renderização iniciada aqui.</small>
            </section>
          </> : <>
            <section className="section-block server-map-install">
              <div className="section-heading compact"><div><span className="overline">Geração de terreno</span><h2>Chunky</h2></div>{mapStackReady ? <CheckCircle2 size={19} /> : <Download size={19} />}</div>
              <div className={`server-inline-warning ${mapStackReady ? "success" : ""}`}><ShieldCheck size={17} /><span><b>{mapStackReady ? "Chunky detectado" : "Download oficial e verificado"}</b><small>O Chunky gera chunks novos; ele não é um visualizador e continua separado do BlueMap.</small></span></div>
              {!mapInstallSupported && <div className="server-inline-warning error"><AlertTriangle size={17} /><span><b>Loader não compatível</b><small>Use Paper ou Fabric para a instalação assistida.</small></span></div>}
              <button className="primary-button" disabled={busy !== null || status.running || !mapInstallSupported || mapStackReady} onClick={() => void installRecommendedMapStack()}><Download size={16} />{busy === "map-install" ? "Baixando..." : mapStackReady ? "Chunky instalado" : "Instalar Chunky"}</button>
              <small className="server-license-note">Chunky: GPL-3.0-only · Fabric API: Apache-2.0.</small>
              {mapInstallResult && mapInstallResultProvider === "chunky" && <div className="server-map-install-result"><b>{mapInstallResult.source}</b>{mapInstallResult.installed.map((item) => <span key={item.destination}><Check size={14} /><em>{item.project} {item.version}</em><small>{item.filename} · {item.license}</small></span>)}</div>}
            </section>
            <section className="section-block server-map-task">
              <div className="section-heading compact"><div><span className="overline">Centro e raio</span><h2>Gerar chunks do mundo</h2></div><Layers3 size={19} /></div>
              <div className="server-map-form"><label>Mundo<input value={mapWorld} onChange={(event) => setMapWorld(event.target.value)} /></label><label>Formato<select value={mapShape} onChange={(event) => setMapShape(event.target.value as "square" | "circle")}><option value="square">Quadrado</option><option value="circle">Circular</option></select></label><label>Coordenada X<input type="number" value={mapX} onChange={(event) => setMapX(Number(event.target.value))} /></label><label>Coordenada Z<input type="number" value={mapZ} onChange={(event) => setMapZ(Number(event.target.value))} /></label><label className="server-map-radius">Raio em blocos<input type="number" min="16" max="50000" value={mapRadius} onChange={(event) => setMapRadius(Number(event.target.value))} /></label></div>
              <button className="secondary-button server-player-location-button" disabled={busy !== null} onClick={() => void useLastPlayerLocation()}><LocateFixed size={16} />Usar última posição salva</button>
              <div className="server-map-actions"><button className="secondary-button" disabled={busy !== null} onClick={() => void previewChunks()}><HardDrive size={16} />Calcular espaço</button><button className="primary-button" disabled={busy !== null || !mapStatus.chunky_installed || !chunkPreview?.safe || !status.running} onClick={() => void startChunks()}><Play size={16} />Gerar chunks</button></div>
              <div className={`server-chunk-preview ${chunkPreview?.safe ? "safe" : ""}`}><span>{mapStatus.chunky_installed ? "Chunky detectado" : "Chunky não encontrado"}</span>{chunkPreview && <><b>{chunkPreview.chunks.toLocaleString("pt-BR")} chunks · ~{chunkPreview.estimated_disk_gb.toFixed(1)} GB</b><small>{chunkPreview.message}</small><code>{chunkPreview.command}</code></>}{!chunkPreview && <small>Calcule antes de liberar. Esta operação cria terreno novo e aumenta o mundo.</small>}</div>
              <div className="server-chunky-controls"><button disabled={busy !== null || !status.running || !mapStatus.chunky_installed} onClick={() => void controlChunks("pause")}><Pause size={14} />Pausar</button><button disabled={busy !== null || !status.running || !mapStatus.chunky_installed} onClick={() => void controlChunks("continue")}><RotateCcw size={14} />Continuar</button><button disabled={busy !== null || !status.running || !mapStatus.chunky_installed} onClick={() => void controlChunks("progress")}><Activity size={14} />Atualizar</button><button className="danger" disabled={busy !== null || !status.running || !mapStatus.chunky_installed} onClick={() => void controlChunks("cancel")}><X size={14} />Cancelar</button></div>
              <div className={`server-map-progress ${mapTask.active ? "active" : ""}`}><span><b>{mapTask.label}</b><small>{mapTask.determinate && typeof mapTask.percent === "number" ? `${mapTask.percent.toFixed(1)}%` : mapTask.active ? "Aguardando medição" : "Em espera"}</small></span><div><i style={{ width: `${mapTask.percent ?? 0}%` }} /></div><p>{mapTask.detail}</p></div>
              {mapMessage && <div className="server-map-message">{mapMessage}</div>}
            </section>
          </>}

          <section className="section-block server-map-storage">
            <div className="section-heading compact"><div><span className="overline">Armazenamento automático</span><h2>Pasta padrão do VEX</h2></div><HardDrive size={19} /></div>
            <div className={`server-inline-warning ${workspaceStorageSafe ? "success" : "error"}`}>{workspaceStorageSafe ? <CheckCircle2 size={17} /> : <AlertTriangle size={17} />}<span><b>{workspaceStorageSafe ? "Disco validado" : "Operação bloqueada"}</b><small>{storagePanelMessage}</small></span></div>
            <div className="config-line"><span>Mundo</span><b>{mapStatus.world_directory || "Ainda não criado"}</b></div><div className="config-line"><span>Dados pesados</span><b>{mapStatus.heavy_storage_directory || "Preparando pasta padrão"}</b></div><div className="config-line"><span>Espaço livre</span><b>{mapStatus.available_disk_gb.toFixed(1)} GB</b></div>
            {mapStatus.heavy_storage_directory && <button className="secondary-button" onClick={() => void serverApi.openPath(mapStatus.heavy_storage_directory!).catch((error) => notify(String(error)))}><FolderOpen size={16} />Abrir pasta</button>}
          </section>
        </aside>
      </div>
    </>}

    {section === "network" && <div className="server-network-grid">
      <section className="section-block server-network-panel">
        <div className="section-heading compact"><div><span className="overline">Túnel externo</span><h2>Playit</h2></div><span className={playit.running ? "online-label" : "offline-label"}>{playit.running ? "Conectado" : playit.service_running ? "Agente ativo" : "Offline"}</span></div>
        <div className="server-playit-hero"><span className={playit.running ? "online" : ""}>{playit.running ? <Wifi size={25} /> : <WifiOff size={25} />}</span><div><b>{playit.running ? "Túnel conectado" : playit.needs_setup ? "Vincule sua conta" : playit.installed ? "Playit detectado" : "Playit não instalado"}</b><small>{playit.detail}</small></div></div>
        {playit.needs_group_access && <div className="server-inline-warning error"><AlertTriangle size={17} /><span><b>Acesso do Linux necessário</b><small>O VEX precisa entrar no grupo playit para conversar com o serviço local.</small></span></div>}
        <label className="server-switch-row"><span><b>Iniciar junto com o servidor</b><small>O VEX tenta abrir o túnel antes do processo do Minecraft.</small></span><input type="checkbox" checked={draftProfile.playit_enabled} onChange={(event) => updateDraft("playit_enabled", event.target.checked)} /></label>
        <div className="server-card-actions">
          {playit.needs_group_access ? <button className="primary-button compact" disabled={busy !== null} onClick={() => void repairPlayitAccess()}><ShieldCheck size={15} />Liberar acesso</button> : playit.needs_setup ? <button className="primary-button compact" disabled={busy !== null} onClick={() => void setupPlayit()}><Globe2 size={15} />Vincular conta</button> : <button className={playit.service_running ? "danger-button compact" : "primary-button compact"} disabled={!playit.installed || busy !== null} onClick={() => void togglePlayit()}>{playit.service_running ? <Square size={14} fill="currentColor" /> : <Play size={15} fill="currentColor" />}{busy === "playit" ? "Aguarde..." : playit.service_running ? "Parar Playit" : "Iniciar Playit"}</button>}
          {draftDirty && <button className="secondary-button compact" disabled={busy !== null || status.running} onClick={() => void saveDraft()}><Save size={15} />{status.running ? "Pare para salvar" : "Salvar preferência"}</button>}
          <button className="secondary-button compact" onClick={() => void serverApi.openUrl("https://playit.gg").catch((error) => notify(String(error)))}><Globe2 size={15} />Abrir playit.gg</button>
        </div>
      </section>

      <section className="section-block server-guide server-network-panel">
        <div className="section-heading compact"><h2>Como seus amigos entram</h2><CircleHelp size={17} /></div>
        <ol><li>Clique em <b>Vincular conta</b> e entre no playit.gg.</li><li>Confirme o agente aberto pelo VEX.</li><li>Crie um túnel Minecraft para <b>localhost:{savedProfile.port}</b>.</li><li>Ative “Iniciar junto com o servidor”.</li><li>Compartilhe o endereço gerado pelo Playit.</li></ol>
        <div className="config-line"><span>Destino local</span><b>localhost:{savedProfile.port}</b></div>
        <div className="config-line"><span>Inicialização automática</span><b>{draftProfile.playit_enabled ? "Ativada" : "Desativada"}</b></div>
      </section>
    </div>}

    {section === "settings" && <section className="section-block server-settings-panel">
      <div className="section-heading"><div><span className="overline">Perfil do servidor</span><h2>Configuração</h2></div><span className={`saved-state ${draftDirty ? "pending" : ""}`}>{draftDirty ? <Settings size={15} /> : <Check size={15} />}{draftDirty ? "Alterações pendentes" : "Salvo"}</span></div>
      {importedProfile && <div className="server-inline-warning"><ShieldCheck size={17} /><span><b>Servidor importado protegido</b><small>O VEX não reescreve server.properties, EULA, mods nem o script existente. Edite esses arquivos na pasta até ativarmos uma revisão explícita de propriedades.</small></span></div>}
      <form className="server-settings-form" onSubmit={(event) => { event.preventDefault(); void saveDraft(); }}>
        <label>Nome<input value={draftProfile.name} onChange={(event) => updateDraft("name", event.target.value)} /></label>
        <label>Versão do Minecraft<input disabled={importedProfile} value={draftProfile.version} onChange={(event) => updateDraft("version", event.target.value)} /></label>
        <label>Software<select disabled={importedProfile} value={draftProfile.software} onChange={(event) => updateDraft("software", event.target.value)}>{!knownSoftwareOptions.includes(draftProfile.software) && <option value={draftProfile.software}>{softwareLabel(draftProfile.software)} (detectado)</option>}<option value="vanilla">Vanilla</option><option value="paper">Paper (plugins)</option><option value="fabric">Fabric (mods)</option></select></label>
        <label>Memória (GB)<input disabled={importedScript} type="number" min="1" max="32" value={draftProfile.memory_gb} onChange={(event) => updateDraft("memory_gb", Number(event.target.value))} /></label>
        <label>Porta<input disabled={importedProfile} type="number" min="1" max="65535" value={draftProfile.port} onChange={(event) => updateDraft("port", Number(event.target.value))} /></label>
        <label>Máximo de jogadores<input disabled={importedProfile} type="number" min="1" value={draftProfile.max_players} onChange={(event) => updateDraft("max_players", Number(event.target.value))} /></label>
        <label>Modo de jogo<select disabled={importedProfile} value={draftProfile.gamemode} onChange={(event) => updateDraft("gamemode", event.target.value)}><option value="survival">Sobrevivência</option><option value="creative">Criativo</option><option value="adventure">Aventura</option><option value="spectator">Espectador</option></select></label>
        <label>Dificuldade<select disabled={importedProfile} value={draftProfile.difficulty} onChange={(event) => updateDraft("difficulty", event.target.value)}><option value="peaceful">Pacífico</option><option value="easy">Fácil</option><option value="normal">Normal</option><option value="hard">Difícil</option></select></label>
        <label className="server-motd-field">Mensagem do servidor<textarea disabled={importedProfile} rows={3} value={draftProfile.motd} onChange={(event) => updateDraft("motd", event.target.value)} /></label>
        <label className="server-check-row"><input disabled={importedProfile} type="checkbox" checked={draftProfile.online_mode} onChange={(event) => updateDraft("online_mode", event.target.checked)} /><span><b>Exigir conta original</b><small>{importedProfile ? "Lido de server.properties; o arquivo foi preservado." : "Mantém online-mode ativado."}</small></span></label>
        <label className="server-check-row"><input type="checkbox" checked={draftProfile.playit_enabled} onChange={(event) => updateDraft("playit_enabled", event.target.checked)} /><span><b>Iniciar Playit automaticamente</b><small>Disponível quando o agente estiver instalado.</small></span></label>
      </form>
      <div className="server-readonly-details">
        <div className="config-line"><span>Pasta</span><b>{draftProfile.directory}</b></div>
        <div className="config-line"><span>Origem</span><b>{draftProfile.managed ? "Criado pelo VEX" : "Servidor encontrado"}</b></div>
        <div className="config-line"><span>Modo de inicialização</span><b>{draftProfile.launch_mode}</b></div>
        {draftProfile.start_target && <div className="config-line"><span>Comando detectado</span><b>{draftProfile.start_target}</b></div>}
      </div>
      <div className="server-settings-actions"><button className="secondary-button" type="button" disabled={!draftDirty || busy === "save"} onClick={discardDraft}>Descartar alterações</button><button className="primary-button" disabled={!draftDirty || status.running || busy === "save"} onClick={() => void saveDraft()}><Save size={16} />{busy === "save" ? "Salvando..." : status.running ? "Pare para salvar" : "Salvar configuração"}</button></div>
    </section>}
    </>}

    {createDialogOpen && <div className="modal-backdrop" onMouseDown={(event) => { if (event.target === event.currentTarget && busy !== "profile") setCreateDialogOpen(false); }}>
      <form className="server-create-modal" onSubmit={(event) => { event.preventDefault(); void createServer(); }}>
        <div className="modal-title-row"><div><span className="overline">Biblioteca de servidores</span><h1>Novo servidor</h1><p>Escolha a base agora. O download acontece somente quando você mandar iniciar.</p></div><IconButton label="Fechar" disabled={busy === "profile"} onClick={() => setCreateDialogOpen(false)}><X size={17} /></IconButton></div>
        <div className="server-create-fields"><label>Nome<input autoFocus value={newServerName} maxLength={96} onChange={(event) => setNewServerName(event.target.value)} /></label><label>Versão exata do Minecraft<input list="server-version-options" value={newServerVersion} maxLength={64} placeholder="26.1.2" onChange={(event) => setNewServerVersion(event.target.value)} /><datalist id="server-version-options">{serverVersionOptions.map((version) => <option key={version} value={version} />)}</datalist></label></div>
        <div className="server-software-choice">
          {([{"id":"vanilla","name":"Vanilla","note":"Servidor original, sem plugins/mods"},{"id":"paper","name":"Paper","note":"Plugins e bom desempenho"},{"id":"fabric","name":"Fabric","note":"Mods e modpacks Fabric"}] as const).map((item) => <label className={newServerSoftware === item.id ? "selected" : ""} key={item.id}><input type="radio" name="server-software" value={item.id} checked={newServerSoftware === item.id} onChange={() => setNewServerSoftware(item.id)} /><span><b>{item.name}</b><small>{item.note}</small></span>{newServerSoftware === item.id && <CheckCircle2 size={17} />}</label>)}
        </div>
        <div className="server-inline-warning"><HardDrive size={17} /><span><b>Pasta gerenciada pelo VEX</b><small>Será criada dentro de {storageRoot}. O VEX nunca usará a pasta do seu servidor importado para este perfil.</small></span></div>
        <div className="modal-actions"><button type="button" className="secondary-button" disabled={busy === "profile"} onClick={() => setCreateDialogOpen(false)}>Cancelar</button><button className="primary-button" disabled={busy !== null || !newServerName.trim() || !newServerVersion.trim()}><Plus size={16} />{busy === "profile" ? "Criando..." : "Criar servidor"}</button></div>
      </form>
    </div>}
  </div>;
}
