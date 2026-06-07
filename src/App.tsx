import {
  Bell,
  Box,
  Check,
  ChevronDown,
  ChevronLeft,
  ChevronRight,
  CircleHelp,
  Code2,
  Compass,
  Download,
  FolderOpen,
  Gamepad2,
  Globe2,
  Home,
  Image,
  Library,
  ListFilter,
  LogOut,
  Maximize2,
  Menu,
  MessageSquareText,
  Minus,
  MoreHorizontal,
  PackagePlus,
  Play,
  Plus,
  Search,
  Server,
  Settings,
  ShieldCheck,
  SlidersHorizontal,
  Sparkles,
  Square,
  TerminalSquare,
  UserRound,
  Users,
  X,
  Zap
} from "lucide-react";
import { useMemo, useRef, useState } from "react";
import { useEffect } from "react";
import { invoke, isTauri } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { projects, type Instance, type Project } from "./data";
import steveFace from "./assets/steve-face.svg";
import vexFace from "./assets/vexface.svg";
import vexLogo from "./assets/vex.svg";

type Page = "home" | "library" | "discover" | "server" | "logs" | "settings";
type DiscoverKind = "Tudo" | "Mods" | "Modpacks" | "Texturas" | "Shaders" | "Plugins";
type BackendInstance = { id: string; name: string; loader: string; mc_version: string; version_id: string; profile_dir: string; icon_path?: string; kind: string; size_mb: number };
type LauncherSettingsResult = { storage_root: string; game_directory: string; offline_username: string; offline_skin_path?: string; use_offline_profile: boolean; onboarding_completed: boolean };
type MicrosoftAccount = { logged_in: boolean; active: boolean; username: string; uuid: string; skin_url?: string };
type JavaRuntime = { path: string; major: number };
type ModrinthInstallTarget = { instance_name: string; game_version: string; loader: string; destination_dir: string; download_url: string; filename: string; sha512?: string };
type InstanceContent = { name: string; path: string; kind: string; size_mb: number; modified_unix: number };
type ServerProfile = { name: string; version: string; software: string; memory_gb: number; port: number; max_players: number; motd: string; online_mode: boolean; gamemode: string; difficulty: string; directory: string };
type ServerStatus = { running: boolean; pid?: number; profile: ServerProfile; log_path: string };
type OperationProgress = { operation: string; label: string; percent: number; done: boolean };

const pageMeta: Record<Page, { title: string; eyebrow: string }> = {
  home: { title: "Início", eyebrow: "Visão geral" },
  library: { title: "Biblioteca", eyebrow: "Suas instâncias" },
  discover: { title: "Descobrir", eyebrow: "Conteúdo da comunidade" },
  server: { title: "Meu servidor", eyebrow: "Servidor local" },
  logs: { title: "Console", eyebrow: "Saída do launcher e Java" },
  settings: { title: "Configurações", eyebrow: "Preferências" }
};

const kinds: DiscoverKind[] = ["Tudo", "Mods", "Modpacks", "Texturas", "Shaders", "Plugins"];

function IconButton({ label, children, className = "", onClick }: { label: string; children: React.ReactNode; className?: string; onClick?: () => void }) {
  return <button className={`icon-button ${className}`} aria-label={label} title={label} onClick={onClick}>{children}</button>;
}

function BrandMark({ small = false, animated = false }: { small?: boolean; animated?: boolean }) {
  return <span className={`brand-mark ${small ? "small" : ""} ${animated ? "animated" : ""}`} aria-label="VEX Launcher"><img src={vexFace} alt="" /></span>;
}

function BootScreen({ progress }: { progress: number }) {
  return (
    <div className="boot-screen">
      <div className="boot-brand">
        <BrandMark animated />
        <img className="boot-wordmark" src={vexLogo} alt="VEX Launcher" />
      </div>
      <div className="boot-status"><span>Preparando seu launcher</span><b>{progress}%</b></div>
      <div className="boot-progress" aria-label={`Abrindo VEX Launcher: ${progress}%`}>
        <span style={{ width: `${progress}%` }}><i /></span>
      </div>
    </div>
  );
}

function ProgressPanel({ progress }: { progress: OperationProgress }) {
  return (
    <div className={`operation-progress ${progress.done ? "done" : ""}`}>
      <div className="operation-progress-copy"><span>{progress.label}</span><b>{progress.percent}%</b></div>
      <div className="operation-progress-track"><span style={{ width: `${progress.percent}%` }} /></div>
    </div>
  );
}

function AccountChoiceModal({ onOffline, onMicrosoft, busy }: { onOffline: () => void; onMicrosoft: () => void; busy: boolean }) {
  return (
    <div className="modal-backdrop">
      <section className="account-choice-modal" role="dialog" aria-modal="true" aria-labelledby="account-choice-title">
        <BrandMark />
        <span className="overline">Bem-vindo ao VEX</span>
        <h1 id="account-choice-title">Como você quer jogar?</h1>
        <p>Você poderá trocar de perfil a qualquer momento nas configurações.</p>
        <div className="account-choice-list">
          <button className="account-choice microsoft" disabled={busy} onClick={onMicrosoft}>
            <span className="account-choice-icon"><ShieldCheck size={22} /></span>
            <span><b>Entrar com Microsoft</b><small>Usa sua conta oficial, nome e skin do Minecraft.</small></span>
            <ChevronRight size={18} />
          </button>
          <button className="account-choice" disabled={busy} onClick={onOffline}>
            <span className="account-choice-icon"><UserRound size={22} /></span>
            <span><b>Jogar offline</b><small>Escolha um nome e uma skin local no VEX.</small></span>
            <ChevronRight size={18} />
          </button>
        </div>
        <small className="privacy-note">O login acontece na página oficial da Microsoft. O VEX nunca recebe sua senha.</small>
      </section>
    </div>
  );
}

function initials(name: string) {
  const parts = name.trim().split(/\s+/).filter(Boolean);
  return (parts.length > 1 ? `${parts[0][0]}${parts[1][0]}` : parts[0]?.slice(0, 2) || "MC").toUpperCase();
}

function SkinFace({ skinDataUrl, label, large = false }: { skinDataUrl?: string; label: string; large?: boolean }) {
  if (!skinDataUrl) return <img className={`skin-face fallback ${large ? "large" : ""}`} src={steveFace} alt={`Rosto de ${label}`} />;
  const style = { backgroundImage: `url("${skinDataUrl}")` };
  return <span className={`skin-face ${large ? "large" : ""}`} role="img" aria-label={`Rosto de ${label}`}><span className="skin-face-base" style={style} /><span className="skin-face-overlay" style={style} /></span>;
}

function Sidebar({ page, setPage, compact, setCompact, username, skinDataUrl, accountLabel, appInstances }: { page: Page; setPage: (p: Page) => void; compact: boolean; setCompact: (v: boolean) => void; username: string; skinDataUrl?: string; accountLabel: string; appInstances: Instance[] }) {
  const items: { id: Page; label: string; icon: React.ReactNode }[] = [
    { id: "home", label: "Início", icon: <Home size={19} /> },
    { id: "library", label: "Biblioteca", icon: <Library size={19} /> },
    { id: "discover", label: "Descobrir", icon: <Compass size={19} /> },
    { id: "server", label: "Meu servidor", icon: <Server size={19} /> },
    { id: "logs", label: "Console", icon: <TerminalSquare size={19} /> }
  ];
  return (
    <aside className={`sidebar ${compact ? "compact" : ""}`}>
      <div className="sidebar-main">
        <div className="sidebar-section-label">Navegação</div>
        {items.map((item) => (
          <button key={item.id} className={`nav-item ${page === item.id ? "active" : ""}`} onClick={() => setPage(item.id)}>
            {item.icon}<span>{item.label}</span>
          </button>
        ))}
        <div className="sidebar-divider" />
        <div className="sidebar-section-label">Instâncias recentes</div>
        {appInstances.slice(0, 3).map((instance) => (
          <button key={instance.id} className="instance-nav" onClick={() => setPage("library")}>
            <span className={`mini-instance-icon ${instance.iconUrl ? "has-image" : ""}`} style={{ background: instance.color }}>{instance.iconUrl ? <img src={instance.iconUrl} alt="" /> : instance.icon}</span>
            <span className="instance-nav-text"><b>{instance.name}</b><small>{instance.version}</small></span>
          </button>
        ))}
        <button className="nav-item secondary" onClick={() => setPage("library")}><Plus size={19} /><span>Nova instância</span></button>
      </div>
      <div className="sidebar-bottom">
        <button className={`nav-item ${page === "settings" ? "active" : ""}`} onClick={() => setPage("settings")}><Settings size={19} /><span>Configurações</span></button>
        <button className="profile-row" onClick={() => setPage("settings")}>
          <SkinFace skinDataUrl={skinDataUrl} label={username} />
          <span className="profile-copy"><b>{username}</b><small>{accountLabel}</small></span>
          <ChevronRight size={17} />
        </button>
        <IconButton label={compact ? "Expandir menu" : "Recolher menu"} className="collapse-sidebar" onClick={() => setCompact(!compact)}>
          {compact ? <ChevronRight size={18} /> : <ChevronLeft size={18} />}
        </IconButton>
      </div>
    </aside>
  );
}

function Topbar({ page, sidebarOpen, setSidebarOpen, notify }: { page: Page; sidebarOpen: boolean; setSidebarOpen: (v: boolean) => void; notify: (message: string) => void }) {
  const [brandBurst, setBrandBurst] = useState(0);
  const windowAction = async (action: "minimize" | "toggleMaximize" | "close") => {
    try {
      const command = action === "minimize" ? "minimize_window" : action === "toggleMaximize" ? "toggle_maximize_window" : "close_window";
      await invoke(command);
    } catch {
      try {
        await getCurrentWindow()[action]();
      } catch {
        // Browser preview has no native window.
      }
    }
  };
  const dragWindow = async (event: React.MouseEvent<HTMLElement>) => {
    if (event.button !== 0 || (event.target as HTMLElement).closest("button")) return;
    try {
      if (event.detail === 2) await getCurrentWindow().toggleMaximize();
      else await getCurrentWindow().startDragging();
    } catch {
      try {
        if (event.detail === 2) await invoke("toggle_maximize_window");
        else await invoke("start_window_dragging");
      } catch {
        // Browser preview has no native window.
      }
    }
  };
  return (
    <header className="titlebar">
      <div className="titlebar-drag-surface" data-tauri-drag-region onMouseDown={(event) => void dragWindow(event)} />
      <div className="titlebar-drag-area" data-tauri-drag-region onMouseDown={(event) => void dragWindow(event)}>
        <div className="titlebar-brand" data-tauri-drag-region>
          <button className="brand-trigger" aria-label="Animar logo VEX" title="VEX Launcher" onClick={() => setBrandBurst((current) => current + 1)}>
            <BrandMark key={brandBurst} small animated={brandBurst > 0} />
          </button>
          <img className="titlebar-wordmark" data-tauri-drag-region src={vexLogo} alt="VEX Launcher" />
        </div>
        <IconButton label="Abrir menu" className="mobile-menu" onClick={() => setSidebarOpen(!sidebarOpen)}><Menu size={18} /></IconButton>
        <div className="breadcrumbs" data-tauri-drag-region><span>{pageMeta[page].eyebrow}</span><ChevronRight size={14} /><b>{pageMeta[page].title}</b></div>
      </div>
      <div className="title-actions">
        <div className="running-state"><span className="status-dot" />Nenhuma instância aberta</div>
        <IconButton label="Notificações" onClick={() => notify("Nenhuma notificação nova")}><Bell size={17} /></IconButton>
        <IconButton label="Minimizar" onClick={() => windowAction("minimize")}><Minus size={16} /></IconButton>
        <IconButton label="Maximizar" onClick={() => windowAction("toggleMaximize")}><Square size={13} /></IconButton>
        <IconButton label="Fechar" className="close" onClick={() => windowAction("close")}><X size={17} /></IconButton>
      </div>
    </header>
  );
}

function InstanceIcon({ instance, large = false }: { instance: Instance; large?: boolean }) {
  return <span className={`instance-icon ${large ? "large" : ""} ${instance.iconUrl ? "has-image" : ""}`} style={{ background: `linear-gradient(145deg, ${instance.color}, #23252c)` }}>{instance.iconUrl ? <img src={instance.iconUrl} alt="" /> : instance.icon}</span>;
}

function ProjectArt({ project, size = "" }: { project: Project; size?: "large" | "hero" | "" }) {
  return (
    <span className={`project-icon ${size}`} style={{ background: project.color }}>
      {project.iconUrl ? <img src={project.iconUrl} alt="" /> : project.icon}
    </span>
  );
}

function HomePage({ play, username, skinDataUrl, accountLabel, appInstances, navigate, gameDirectory, notify }: { play: (instance: Instance) => void; username: string; skinDataUrl?: string; accountLabel: string; appInstances: Instance[]; navigate: (page: Page) => void; gameDirectory: string; notify: (message: string) => void }) {
  const active = appInstances[0];
  const openGameFolder = async () => {
    try {
      await invoke("open_path", { path: gameDirectory });
    } catch (error) {
      notify(`Não foi possível abrir a pasta: ${String(error)}`);
    }
  };
  if (!active) {
    return <div className="page-grid home-grid empty-home"><section className="hero-panel"><div className="hero-copy"><span className="overline"><Sparkles size={14} /> Primeiro passo</span><h1>Boa tarde, {username}.</h1><p>Crie uma instância Vanilla ou Fabric para começar a jogar e instalar conteúdo.</p><div className="hero-actions"><button className="primary-button" onClick={() => navigate("library")}><Plus size={18} />Criar instância</button><button className="secondary-button" onClick={() => navigate("discover")}><Compass size={18} />Descobrir conteúdo</button></div></div></section></div>;
  }
  return (
    <div className="page-grid home-grid">
      <section className="hero-panel">
        <div className="hero-copy">
          <span className="overline"><Sparkles size={14} /> Pronto para jogar</span>
          <h1>Boa tarde, {username}.</h1>
          <p>Seu mundo mais recente está pronto. Configurações, perfil e conteúdo já foram verificados.</p>
          <div className="hero-actions">
            <button className="primary-button" onClick={() => play(active)}><Play size={18} fill="currentColor" />Jogar {active.name}</button>
            <IconButton label="Configurar instância" onClick={() => navigate("library")}><Settings size={18} /></IconButton>
            <IconButton label="Abrir pasta da instância" onClick={() => void invoke("open_path", { path: active.profileDir ?? gameDirectory }).catch((error) => notify(String(error)))}><FolderOpen size={19} /></IconButton>
          </div>
        </div>
        <div className="hero-instance">
          <InstanceIcon instance={active} large />
          <div><b>{active.name}</b><span>{active.loader} · Minecraft {active.version}</span></div>
          <span className="verified"><ShieldCheck size={14} /> Verificada</span>
        </div>
      </section>
      <section className="section-block recent-block">
        <div className="section-heading"><div><span className="overline">Biblioteca</span><h2>Jogados recentemente</h2></div><button className="text-button" onClick={() => navigate("library")}>Ver todos <ChevronRight size={16} /></button></div>
        <div className="recent-list">
          {appInstances.slice(1, 4).map((instance) => (
            <article className="recent-row" key={instance.id}>
              <InstanceIcon instance={instance} />
              <div className="grow"><b>{instance.name}</b><span>{instance.loader} · {instance.version} · {instance.mods} mods</span></div>
              <span className="muted-time">{instance.lastPlayed}</span>
              <IconButton label={`Jogar ${instance.name}`} onClick={() => play(instance)}><Play size={16} fill="currentColor" /></IconButton>
            </article>
          ))}
        </div>
      </section>
      <aside className="home-side">
        <section className="section-block profile-card">
          <div className="section-heading compact"><h2>Perfil ativo</h2><IconButton label="Editar perfil" onClick={() => navigate("settings")}><Settings size={16} /></IconButton></div>
          <div className="skin-preview"><SkinFace skinDataUrl={skinDataUrl} label={username} large /></div>
          <div className="profile-center"><b>{username}</b><span>{accountLabel} · {skinDataUrl ? "Skin ativa" : "Skin clássica"}</span></div>
          <div className="profile-check"><Check size={15} /> Será usado em todas as instâncias</div>
        </section>
        <section className="section-block quick-card">
          <div className="section-heading compact"><h2>Ações rápidas</h2></div>
          <button onClick={() => navigate("discover")}><PackagePlus size={17} /><span><b>Adicionar conteúdo</b><small>Mods, shaders e texturas</small></span><ChevronRight size={16} /></button>
          <button onClick={() => void openGameFolder()}><FolderOpen size={17} /><span><b>Abrir pasta do jogo</b><small>{gameDirectory}</small></span><ChevronRight size={16} /></button>
          <button onClick={() => navigate("logs")}><TerminalSquare size={17} /><span><b>Ver logs</b><small>Console do launcher e Java</small></span><ChevronRight size={16} /></button>
        </section>
      </aside>
    </div>
  );
}

function LibraryPage({ play, appInstances, navigate, refreshInstances, notify }: { play: (instance: Instance) => void; appInstances: Instance[]; navigate: (page: Page) => void; refreshInstances: () => Promise<void>; notify: (message: string) => void }) {
  const [selectedId, setSelectedId] = useState(appInstances[0]?.id ?? "");
  const [tab, setTab] = useState("Conteúdo");
  const [query, setQuery] = useState("");
  const [contentQuery, setContentQuery] = useState("");
  const [onlyModded, setOnlyModded] = useState(false);
  const [content, setContent] = useState<InstanceContent[]>([]);
  const selected = appInstances.find((instance) => instance.id === selectedId) ?? appInstances[0];
  const filteredInstances = appInstances.filter((instance) => instance.name.toLowerCase().includes(query.toLowerCase()) && (!onlyModded || instance.loader.toLowerCase() !== "vanilla"));
  const visibleContent = content.filter((item) => item.name.toLowerCase().includes(contentQuery.toLowerCase()));

  useEffect(() => {
    if (!selected?.profileDir) {
      setContent([]);
      return;
    }
    invoke<InstanceContent[]>("list_instance_content", { profileDir: selected.profileDir, category: tab }).then(setContent).catch((error) => notify(String(error)));
  }, [selected?.profileDir, tab]);

  const createInstance = async () => {
    const name = window.prompt("Nome da nova instância:", "Minha aventura")?.trim();
    if (!name) return;
    const version = window.prompt("Versão do Minecraft:", "1.21.4")?.trim();
    if (!version) return;
    try {
      const loader = window.prompt("Loader: vanilla ou fabric", "fabric")?.trim().toLowerCase();
      if (!loader) return;
      const created = await invoke<BackendInstance>("create_instance", { name, version, loader });
      await refreshInstances();
      setSelectedId(created.id);
      notify(`${created.name} criada`);
    } catch (error) {
      notify(`Falha ao criar instância: ${String(error)}`);
    }
  };
  const reloadContent = () => {
    if (selected?.profileDir) invoke<InstanceContent[]>("list_instance_content", { profileDir: selected.profileDir, category: tab }).then(setContent).catch((error) => notify(String(error)));
  };
  const removeContent = async (item: InstanceContent) => {
    if (!window.confirm(`Remover "${item.name}"?`)) return;
    try {
      await invoke("remove_instance_content", { path: item.path });
      reloadContent();
      notify(`${item.name} removido`);
    } catch (error) {
      notify(`Não foi possível remover: ${String(error)}`);
    }
  };

  return (
    <div className="library-layout">
      <section className="instance-list-panel">
        <div className="page-intro"><div><span className="overline">{appInstances.length} instância(s)</span><h1>Biblioteca</h1></div><button className="primary-button small" onClick={() => void createInstance()}><Plus size={17} />Nova</button></div>
        <div className="search-field"><Search size={17} /><input aria-label="Buscar instâncias" placeholder="Buscar instâncias..." value={query} onChange={(event) => setQuery(event.target.value)} /><button className={`bare-filter ${onlyModded ? "active" : ""}`} title="Mostrar apenas instâncias com loader" onClick={() => setOnlyModded(!onlyModded)}><SlidersHorizontal size={16} /></button></div>
        <div className="stack-list">
          {filteredInstances.map((instance) => (
            <button className={`instance-list-item ${selected?.id === instance.id ? "active" : ""}`} key={`${instance.id}-${instance.profileDir}`} onClick={() => setSelectedId(instance.id)}>
              <InstanceIcon instance={instance} />
              <span className="grow"><b>{instance.name}</b><small>{instance.loader} · {instance.version}</small></span>
              <span className="item-status">{instance.mods || "Vanilla"}</span>
            </button>
          ))}
          {!filteredInstances.length && <div className="empty-state"><Box size={21} /><span>Nenhuma instância encontrada.</span></div>}
        </div>
      </section>
      <section className="instance-detail">
        {selected ? <>
          <div className="instance-detail-head">
            <InstanceIcon instance={selected} large />
            <div className="grow"><span className="overline">{selected.loader} · {selected.version}</span><h1>{selected.name}</h1><p>{selected.lastPlayed}</p></div>
            <button className="primary-button" onClick={() => play(selected)}><Play size={18} fill="currentColor" />Jogar</button>
            <IconButton label="Abrir pasta da instância" onClick={() => void invoke("open_path", { path: selected.profileDir }).catch((error) => notify(String(error)))}><FolderOpen size={19} /></IconButton>
          </div>
          <div className="tabs">
            {["Conteúdo", "Mundos", "Capturas", "Logs"].map((item) => <button key={item} className={tab === item ? "active" : ""} onClick={() => setTab(item)}>{item}</button>)}
          </div>
          <div className="content-toolbar">
            <div className="search-field grow"><Search size={17} /><input aria-label="Buscar conteúdo" placeholder={`Buscar em ${tab.toLowerCase()}...`} value={contentQuery} onChange={(event) => setContentQuery(event.target.value)} /></div>
            <button className="secondary-button" onClick={() => navigate("discover")}><Download size={17} />Adicionar conteúdo</button>
            <IconButton label="Atualizar lista" onClick={reloadContent}><ListFilter size={18} /></IconButton>
          </div>
          <div className="content-table">
            <div className="table-head"><span>Nome</span><span>Tamanho</span><span>Tipo</span><span /></div>
            {visibleContent.map((item) => (
              <div className="table-row" key={item.path}>
                <span className="project-icon" style={{ background: "#587180" }}>{item.kind.slice(0, 2)}</span>
                <button className="grow row-main-button" onClick={() => void invoke("open_path", { path: item.path }).catch((error) => notify(String(error)))}><b>{item.name}</b><small>Abrir no Explorador</small></button>
                <span className="table-version">{item.size_mb} MB</span>
                <span className="enabled-state"><span />{item.kind}</span>
                <IconButton label={`Remover ${item.name}`} onClick={() => void removeContent(item)}><X size={18} /></IconButton>
              </div>
            ))}
            {!visibleContent.length && <div className="empty-state"><Box size={21} /><span>Nenhum item em {tab.toLowerCase()}.</span></div>}
          </div>
        </> : <div className="empty-state large-empty"><Box size={28} /><span>Crie uma instância para começar.</span><button className="primary-button" onClick={() => void createInstance()}><Plus size={17} />Nova instância</button></div>}
      </section>
    </div>
  );
}

function DiscoverPage({ appInstances }: { appInstances: Instance[] }) {
  const [kind, setKind] = useState<DiscoverKind>("Modpacks");
  const [selected, setSelected] = useState<Project | null>(null);
  const [query, setQuery] = useState("");
  const [remoteProjects, setRemoteProjects] = useState<Project[] | null>(null);
  const [loading, setLoading] = useState(false);
  const [searchError, setSearchError] = useState("");
  const [filtersOpen, setFiltersOpen] = useState(false);
  const [sortIndex, setSortIndex] = useState<"relevance" | "downloads" | "updated">("downloads");
  const [page, setPage] = useState(1);
  const [pageSize, setPageSize] = useState(20);
  const [totalHits, setTotalHits] = useState(0);
  const shown = useMemo(() => remoteProjects ?? projects.filter((project) => kind === "Tudo" || project.kind === kind.slice(0, -1) || project.kind === kind), [kind, remoteProjects]);
  const totalPages = Math.max(1, Math.ceil(totalHits / pageSize));
  const visiblePages = useMemo(() => {
    const start = Math.max(1, Math.min(page - 2, totalPages - 4));
    return Array.from({ length: Math.min(5, totalPages) }, (_, index) => start + index);
  }, [page, totalPages]);
  const kindMap: Record<DiscoverKind, string | null> = { Tudo: null, Mods: "mod", Modpacks: "modpack", Texturas: "resourcepack", Shaders: "shader", Plugins: "plugin" };
  const searchProjects = async (selectedKind = kind, selectedSort = sortIndex, selectedPage = page, selectedPageSize = pageSize) => {
    setLoading(true);
    setSearchError("");
    setPage(selectedPage);
    try {
      const facets = kindMap[selectedKind] ? `&facets=${encodeURIComponent(JSON.stringify([[`project_type:${kindMap[selectedKind]}`]]))}` : "";
      const offset = (selectedPage - 1) * selectedPageSize;
      const response = await fetch(`https://api.modrinth.com/v2/search?query=${encodeURIComponent(query.trim())}&limit=${selectedPageSize}&offset=${offset}&index=${selectedSort}${facets}`);
      if (!response.ok) throw new Error(`Modrinth respondeu ${response.status}`);
      const data = await response.json() as { hits: Array<Record<string, unknown>>; total_hits?: number };
      setTotalHits(Number(data.total_hits ?? data.hits.length));
      setRemoteProjects(data.hits.map((hit, index) => ({
        id: String(hit.project_id),
        name: String(hit.title),
        author: String(hit.author),
        kind: String(hit.project_type) === "resourcepack" ? "Textura" : String(hit.project_type) === "modpack" ? "Modpack" : String(hit.project_type) === "shader" ? "Shader" : String(hit.project_type) === "plugin" ? "Plugin" : "Mod",
        description: String(hit.description ?? ""),
        versions: Array.isArray(hit.versions) ? hit.versions.map(String).slice(-8).reverse() : [],
        downloads: new Intl.NumberFormat("pt-BR", { notation: "compact", maximumFractionDigits: 1 }).format(Number(hit.downloads ?? 0)),
        color: ["#607f91", "#6f9295", "#8a929c", "#647d9a", "#68a37a"][index % 5],
        icon: String(hit.title).slice(0, 2),
        iconUrl: typeof hit.icon_url === "string" ? hit.icon_url : undefined
      })));
    } catch (error) {
      setSearchError(error instanceof Error ? error.message : "Não foi possível pesquisar agora.");
      setRemoteProjects(null);
      setTotalHits(0);
    } finally {
      setLoading(false);
    }
  };
  useEffect(() => {
    void searchProjects("Modpacks", "downloads", 1, 20);
  }, []);
  const openProject = async (project: Project) => {
    setSelected(project);
    try {
      const response = await fetch(`https://api.modrinth.com/v2/project/${project.id}/version`);
      if (!response.ok) return;
      const versions = await response.json() as Array<{ game_versions?: string[] }>;
      const available = [...new Set(versions.flatMap((version) => version.game_versions ?? []))].slice(0, 12);
      setSelected({ ...project, versions: available.length ? available : project.versions });
    } catch {
      // The project page still works with versions returned by search.
    }
  };
  if (selected) return <ProjectDetail project={selected} close={() => setSelected(null)} appInstances={appInstances} />;
  return (
    <div className="discover-page">
      <div className="page-intro discover-intro"><div><span className="overline">Modrinth e fontes compatíveis</span><h1>Descubra seu próximo mundo</h1><p>Conteúdo organizado por compatibilidade, versão e tipo.</p></div></div>
      <div className="discover-toolbar">
        <div className="search-field large grow"><Search size={19} /><input aria-label="Buscar projetos" placeholder="Buscar mods, modpacks, texturas e shaders..." value={query} onChange={(event) => setQuery(event.target.value)} onKeyDown={(event) => { if (event.key === "Enter") void searchProjects(kind, sortIndex, 1); }} /><kbd>Enter</kbd></div>
        <button className="primary-button search-button" onClick={() => void searchProjects(kind, sortIndex, 1)}><Search size={17} />Buscar</button>
        <button className={`secondary-button ${filtersOpen ? "following" : ""}`} onClick={() => setFiltersOpen(!filtersOpen)}><SlidersHorizontal size={17} />Filtros</button>
      </div>
      {filtersOpen && <div className="filter-panel"><span>Tipo de conteúdo</span><b>{kind}</b><span>Compatibilidade</span><b>{appInstances.length} instância(s) instalada(s)</b><button className="text-button" onClick={() => { setKind("Modpacks"); setQuery(""); setSortIndex("downloads"); setFiltersOpen(false); void searchProjects("Modpacks", "downloads", 1); }}>Limpar filtros</button></div>}
      <div className="category-strip">
        {kinds.map((item) => <button key={item} className={kind === item ? "active" : ""} onClick={() => { setKind(item); void searchProjects(item, sortIndex, 1); }}>{item}</button>)}
      </div>
      <div className="results-heading">
        <div><b>{loading ? "Buscando no Modrinth..." : query.trim() ? `Resultados para “${query}”` : kind === "Modpacks" && sortIndex === "downloads" ? "Modpacks populares do momento" : `${kind} em destaque`}</b><span>{searchError || `${totalHits.toLocaleString("pt-BR")} projetos encontrados`}</span></div>
        <div className="results-controls">
          <label className="page-size-control">Por página<select aria-label="Itens por página" value={pageSize} onChange={(event) => { const nextSize = Number(event.target.value); setPageSize(nextSize); void searchProjects(kind, sortIndex, 1, nextSize); }}><option value={20}>20</option><option value={40}>40</option><option value={60}>60</option><option value={100}>100</option></select></label>
          <button className="sort-button" onClick={() => { const next = sortIndex === "relevance" ? "downloads" : sortIndex === "downloads" ? "updated" : "relevance"; setSortIndex(next); void searchProjects(kind, next, 1); }}>{sortIndex === "relevance" ? "Mais relevantes" : sortIndex === "downloads" ? "Mais baixados" : "Atualizados"} <ChevronDown size={16} /></button>
        </div>
      </div>
      <div className="project-grid">
        {shown.map((project) => (
          <button className="project-card" key={project.id} onClick={() => void openProject(project)}>
            <ProjectArt project={project} size="large" />
            <span className="project-copy"><span className="project-kind">{project.kind}</span><b>{project.name}</b><small>por {project.author}</small><p>{project.description}</p></span>
            <span className="project-footer"><span><Download size={14} />{project.downloads}</span><span>{project.versions[0]}</span></span>
          </button>
        ))}
      </div>
      {!loading && totalPages > 1 && <nav className="pagination" aria-label="Páginas de resultados">
        <button aria-label="Página anterior" disabled={page === 1} onClick={() => void searchProjects(kind, sortIndex, page - 1)}><ChevronLeft size={16} /></button>
        {visiblePages.map((item) => <button key={item} className={item === page ? "active" : ""} aria-current={item === page ? "page" : undefined} onClick={() => void searchProjects(kind, sortIndex, item)}>{item}</button>)}
        <button aria-label="Próxima página" disabled={page === totalPages} onClick={() => void searchProjects(kind, sortIndex, page + 1)}><ChevronRight size={16} /></button>
        <span>Página {page} de {totalPages.toLocaleString("pt-BR")}</span>
      </nav>}
    </div>
  );
}

function ProjectDetail({ project, close, appInstances }: { project: Project; close: () => void; appInstances: Instance[] }) {
  const [targets, setTargets] = useState<ModrinthInstallTarget[]>([]);
  const [selectedVersion, setSelectedVersion] = useState("");
  const [installMessage, setInstallMessage] = useState("");
  const [loadingTargets, setLoadingTargets] = useState(false);
  const projectType = project.kind === "Textura" ? "resourcepack" : project.kind === "Shader" ? "shader" : project.kind === "Modpack" ? "modpack" : project.kind === "Plugin" ? "plugin" : "mod";
  const findTargets = async (version: string) => {
    setSelectedVersion(version);
    setLoadingTargets(true);
    setInstallMessage("");
    if (projectType === "modpack") {
      try {
        setInstallMessage(`Instalando ${project.name}. Isso pode levar alguns minutos...`);
        await invoke("install_modrinth_modpack", { projectId: project.id, projectName: project.name, author: project.author, gameVersion: version });
        setInstallMessage(`${project.name} instalado. Atualizando a Biblioteca...`);
        window.setTimeout(() => window.location.reload(), 1800);
      } catch (error) {
        setInstallMessage(`Falha na instalação: ${String(error)}`);
      } finally {
        setLoadingTargets(false);
      }
      return;
    }
    try {
      const compatible = await invoke<ModrinthInstallTarget[]>("get_modrinth_install_targets", { projectId: project.id, projectType, gameVersion: version });
      setTargets(compatible);
      if (!compatible.length) setInstallMessage(`Nenhuma instância instalada aceita Minecraft ${version}.`);
    } catch (error) {
      setTargets([]);
      setInstallMessage(String(error));
    } finally {
      setLoadingTargets(false);
    }
  };
  const installTarget = async (target: ModrinthInstallTarget) => {
    setInstallMessage(`Instalando ${project.name} em ${target.instance_name}...`);
    try {
      const destination = await invoke<string>("install_modrinth_target", { target });
      setInstallMessage(`Instalado com segurança em ${destination}`);
    } catch (error) {
      setInstallMessage(`Falha na instalação: ${String(error)}`);
    }
  };
  return (
    <div className="project-detail-page">
      <button className="back-button" onClick={close}><ChevronLeft size={17} />Voltar para resultados</button>
      <section className="project-detail-hero">
        <ProjectArt project={project} size="hero" />
        <div className="grow"><span className="overline">{project.kind} · por {project.author}</span><h1>{project.name}</h1><p>{project.description}</p><div className="project-stats"><span><Download size={15} />{project.downloads} downloads</span><span><ShieldCheck size={15} />Projeto verificado</span></div></div>
        <button className="primary-button" onClick={() => void findTargets(project.versions[0])}><Download size={18} />Instalar</button>
      </section>
      <div className="detail-columns">
        <section className="section-block versions-block">
          <div className="section-heading"><div><span className="overline">Compatibilidade</span><h2>Versões disponíveis</h2></div><div className="search-field short"><Search size={16} /><input placeholder="Filtrar versões" /></div></div>
          {project.versions.map((version, index) => (
            <div className={`version-row ${selectedVersion === version ? "selected" : ""}`} key={version}><span className="version-symbol"><Box size={17} /></span><span className="grow"><b>{version}</b><small>{index === 0 ? "Versão recomendada" : "Versão estável"}</small></span><span className="version-tags"><i>Minecraft {version}</i></span><button className="secondary-button compact" onClick={() => void findTargets(version)}><Download size={15} />Instalar</button></div>
          ))}
        </section>
        <aside className="section-block install-aside"><span className="overline">Instalação inteligente</span><h2>{selectedVersion ? `Minecraft ${selectedVersion}` : "Escolha uma versão"}</h2><p>{selectedVersion ? "Somente destinos realmente compatíveis aparecem abaixo." : "Selecione uma versão para encontrar instâncias compatíveis."}</p>
          {loadingTargets && <div className="empty-state"><span className="toast-spinner" /><span>Consultando arquivos do Modrinth...</span></div>}
          {!loadingTargets && targets.map((target) => <div className="compatible-target" key={target.destination_dir}><span className="mini-instance-icon" style={{ background: "#587180" }}>{initials(target.instance_name)}</span><span className="grow"><b>{target.instance_name}</b><small>{target.loader} · {target.game_version}</small></span><button className="secondary-button compact" onClick={() => void installTarget(target)}>Instalar</button></div>)}
          {!loadingTargets && !targets.length && !installMessage && <div className="empty-state"><Box size={20} /><span>Escolha uma versão para começar.</span></div>}
          {installMessage && <div className={`install-message ${installMessage.startsWith("Instalado") ? "success" : ""}`}>{installMessage}</div>}
        </aside>
      </div>
    </div>
  );
}

function ServerPage({ notify, storageRoot }: { notify: (message: string) => void; storageRoot: string }) {
  const fallback: ServerProfile = { name: "Meu servidor", version: "1.21.4", software: "vanilla", memory_gb: 4, port: 25565, max_players: 12, motd: "Servidor criado pelo VEX Launcher", online_mode: true, gamemode: "survival", difficulty: "normal", directory: `${storageRoot}\\servers\\Meu servidor` };
  const [profile, setProfile] = useState<ServerProfile>(fallback);
  const [running, setRunning] = useState(false);
  const [log, setLog] = useState("O servidor ainda não foi iniciado.");
  const [command, setCommand] = useState("");
  const [editing, setEditing] = useState(false);
  const [busy, setBusy] = useState(false);
  const refresh = () => {
    invoke<ServerStatus>("server_status").then((status) => { setRunning(status.running); setProfile(status.profile); }).catch(() => undefined);
    invoke<string>("read_server_log").then((content) => setLog(content || "O console está vazio.")).catch(() => undefined);
  };
  useEffect(() => {
    invoke<ServerProfile>("get_server_profile").then(setProfile).catch(() => undefined);
    refresh();
    const interval = window.setInterval(refresh, 1200);
    return () => window.clearInterval(interval);
  }, []);
  const save = async () => {
    try {
      const saved = await invoke<ServerProfile>("save_server_profile", { profile });
      setProfile(saved);
      setEditing(false);
      notify("Configuração do servidor salva");
    } catch (error) {
      notify(`Falha ao salvar servidor: ${String(error)}`);
    }
  };
  const toggleServer = async () => {
    setBusy(true);
    try {
      if (running) await invoke("stop_server");
      else await invoke("start_server");
      refresh();
      notify(running ? "Comando de desligamento enviado" : "Servidor iniciando");
    } catch (error) {
      notify(`Falha no servidor: ${String(error)}`);
    } finally {
      setBusy(false);
    }
  };
  const send = async () => {
    if (!command.trim()) return;
    try {
      await invoke("send_server_command", { command });
      setCommand("");
    } catch (error) {
      notify(String(error));
    }
  };
  const clear = async () => {
    try {
      await invoke("clear_server_log");
      setLog("O console está vazio.");
    } catch (error) {
      notify(String(error));
    }
  };
  return (
    <div className="server-page">
      <div className="page-intro"><div><span className="overline">Controle local</span><h1>{profile.name}</h1><p>Servidor Vanilla local, configurável e controlado pelo launcher.</p></div><button disabled={busy} className={running ? "danger-button" : "primary-button"} onClick={() => void toggleServer()}>{running ? <Square size={16} fill="currentColor" /> : <Play size={17} fill="currentColor" />}{busy ? "Aguarde..." : running ? "Parar servidor" : "Iniciar servidor"}</button></div>
      <div className="server-status-band">
        <span className={`server-power ${running ? "online" : ""}`}><Zap size={21} /></span>
        <div className="grow"><b>{running ? "Servidor online" : "Servidor parado"}</b><span>{running ? `localhost:${profile.port}` : "Ao iniciar, o launcher baixa o servidor oficial e abre o console."}</span></div>
        <div className="server-stat"><span>Versão</span><b>{profile.version} {profile.software}</b></div>
        <div className="server-stat"><span>Memória</span><b>{profile.memory_gb} GB alocados</b></div>
        <div className="server-stat"><span>Jogadores</span><b>Até {profile.max_players}</b></div>
      </div>
      <div className="server-columns">
        <section className="section-block console-block">
          <div className="section-heading compact"><div><span className="overline">Saída em tempo real</span><h2>Console</h2></div><div className="console-actions"><span className={running ? "online-label" : "offline-label"}>{running ? "Online" : "Offline"}</span><IconButton label="Limpar console" onClick={() => void clear()}><X size={16} /></IconButton></div></div>
          <pre className="console-output server-log">{log}</pre>
          <div className="console-input"><span>&gt;</span><input value={command} onChange={(event) => setCommand(event.target.value)} onKeyDown={(event) => { if (event.key === "Enter") void send(); }} placeholder={running ? "Digite um comando..." : "Inicie o servidor para usar o console"} disabled={!running} /><button disabled={!running} onClick={() => void send()}>Enviar</button></div>
        </section>
        <aside className="server-side-stack">
          <section className="section-block">
            <div className="section-heading compact"><h2>Acesso</h2><IconButton label="Abrir pasta do servidor" onClick={() => void invoke("open_path", { path: profile.directory }).catch((error) => notify(String(error)))}><FolderOpen size={16} /></IconButton></div>
            <div className="config-line"><span>Endereço local</span><b>localhost:{profile.port}</b></div><div className="config-line"><span>Conta original</span><b>{profile.online_mode ? "Obrigatória" : "Opcional"}</b></div><div className="empty-state compact-empty"><Users size={22} /><span>Jogadores conectados aparecem no próprio console.</span></div>
          </section>
          <section className="section-block">
            <div className="section-heading compact"><h2>Configuração</h2><IconButton label={editing ? "Fechar edição" : "Editar servidor"} onClick={() => setEditing(!editing)}>{editing ? <X size={16} /> : <Settings size={16} />}</IconButton></div>
            {editing ? <div className="server-form">
              <label>Nome<input value={profile.name} onChange={(event) => setProfile({ ...profile, name: event.target.value })} /></label>
              <label>Versão<input value={profile.version} onChange={(event) => setProfile({ ...profile, version: event.target.value })} /></label>
              <label>Software<select value={profile.software} onChange={(event) => setProfile({ ...profile, software: event.target.value })}><option value="vanilla">Vanilla</option><option value="paper">Paper (plugins)</option><option value="fabric">Fabric (mods)</option></select></label>
              <label>Memória (GB)<input type="number" min="1" max="32" value={profile.memory_gb} onChange={(event) => setProfile({ ...profile, memory_gb: Number(event.target.value) })} /></label>
              <label>Porta<input type="number" min="1" max="65535" value={profile.port} onChange={(event) => setProfile({ ...profile, port: Number(event.target.value) })} /></label>
              <label>Máximo de jogadores<input type="number" min="1" value={profile.max_players} onChange={(event) => setProfile({ ...profile, max_players: Number(event.target.value) })} /></label>
              <label className="check-row"><input type="checkbox" checked={profile.online_mode} onChange={(event) => setProfile({ ...profile, online_mode: event.target.checked })} />Exigir conta original</label>
              <button className="primary-button" disabled={running} onClick={() => void save()}><Check size={16} />Salvar</button>
            </div> : <><div className="config-line"><span>Software</span><b>{profile.software}</b></div><div className="config-line"><span>Modo</span><b>{profile.gamemode}</b></div><div className="config-line"><span>Online mode</span><b>{profile.online_mode ? "Ativado" : "Desativado"}</b></div><div className="config-line"><span>Dificuldade</span><b>{profile.difficulty}</b></div><div className="config-line"><span>Porta</span><b>{profile.port}</b></div></>}
          </section>
        </aside>
      </div>
    </div>
  );
}

function LogsPage({ storageRoot }: { storageRoot: string }) {
  const [content, setContent] = useState("Nenhum processo iniciado nesta sessão.");
  const [following, setFollowing] = useState(true);
  const refresh = () => invoke<string>("read_latest_log").then((log) => setContent(log || "O arquivo de log ainda está vazio.")).catch(() => undefined);
  useEffect(() => {
    refresh();
    if (!following) return;
    const interval = window.setInterval(refresh, 1200);
    return () => window.clearInterval(interval);
  }, [following]);
  return (
    <div className="logs-page">
      <div className="page-intro"><div><span className="overline">Saída ao vivo</span><h1>Console</h1><p>Mensagens do launcher, Minecraft e Java aparecem aqui.</p></div><div className="button-row"><button className={`secondary-button ${following ? "following" : ""}`} onClick={() => setFollowing(!following)}><span className="status-dot" />{following ? "Acompanhando" : "Pausado"}</button><button className="secondary-button" onClick={refresh}><TerminalSquare size={16} />Atualizar</button></div></div>
      <section className="section-block live-console">
        <div className="section-heading compact"><div><span className="overline">{storageRoot}\logs\latest.log</span><h2>Última execução</h2></div><span className="online-label">Somente leitura</span></div>
        <pre>{content}</pre>
      </section>
    </div>
  );
}

function SettingsPage({ username, skinDataUrl, onSkinChanged, onSaveProfile, microsoftAccount, useOfflineProfile, onMicrosoftLogin, onUseMicrosoft, onUseOffline, onMicrosoftLogout, storageRoot, gameDirectory, javaRuntimes, onGameDirectoryChanged, notify }: { username: string; skinDataUrl?: string; onSkinChanged: (dataUrl?: string) => void; onSaveProfile: (username: string) => Promise<void>; microsoftAccount: MicrosoftAccount; useOfflineProfile: boolean; onMicrosoftLogin: () => void; onUseMicrosoft: () => void; onUseOffline: () => void; onMicrosoftLogout: () => void; storageRoot: string; gameDirectory: string; javaRuntimes: JavaRuntime[]; onGameDirectoryChanged: (path: string) => void; notify: (message: string) => void }) {
  const [draftName, setDraftName] = useState(username);
  const [saved, setSaved] = useState(true);
  const [skinStatus, setSkinStatus] = useState("Nenhuma skin personalizada");
  const [section, setSection] = useState<"profile" | "minecraft" | "network" | "appearance" | "advanced" | "help">("profile");
  const [dense, setDense] = useState(false);
  const skinInput = useRef<HTMLInputElement>(null);
  useEffect(() => setDraftName(username), [username]);
  const saveProfile = async () => {
    const cleanName = draftName.trim();
    if (!/^[A-Za-z0-9_]{3,16}$/.test(cleanName)) return;
    await onSaveProfile(cleanName);
    setSaved(true);
  };
  const saveSkin = async (file?: File) => {
    if (!file) return;
    try {
      const bytes = Array.from(new Uint8Array(await file.arrayBuffer()));
      const path = await invoke<string>("save_offline_skin", { bytes });
      onSkinChanged(await invoke<string>("read_image_data_url", { path }));
      onUseOffline();
      setSkinStatus(`${file.name} salva no perfil global`);
    } catch (error) {
      setSkinStatus(typeof error === "string" ? error : "Skin inválida. Use PNG 64×64 ou 64×32.");
    }
  };
  const removeSkin = async () => {
    await invoke("remove_offline_skin").catch(() => undefined);
    onSkinChanged(undefined);
    setSkinStatus("Nenhuma skin personalizada");
  };
  const openUrl = (url: string) => invoke("open_url", { url }).catch((error) => notify(String(error)));
  const changeDirectory = async () => {
    const path = window.prompt("Pasta do Minecraft:", gameDirectory)?.trim();
    if (!path || path === gameDirectory) return;
    try {
      const result = await invoke<LauncherSettingsResult>("set_game_directory", { gameDirectory: path });
      onGameDirectoryChanged(result.game_directory);
      notify("Pasta do Minecraft atualizada");
    } catch (error) {
      notify(`Não foi possível alterar a pasta: ${String(error)}`);
    }
  };
  const clearCache = async () => {
    try {
      const bytes = await invoke<number>("clear_launcher_cache");
      notify(`${(bytes / 1_048_576).toFixed(1)} MB removidos do cache`);
    } catch (error) {
      notify(String(error));
    }
  };
  const logout = async () => {
    if (!window.confirm("Voltar para o perfil offline Player?")) return;
    await removeSkin();
    setDraftName("Player");
    await onSaveProfile("Player");
    notify("Perfil offline redefinido");
  };
  return (
    <div className="settings-page">
      <div className="page-intro"><div><span className="overline">Preferências</span><h1>Configurações</h1><p>O launcher aplica estas escolhas a todas as instâncias.</p></div></div>
      <div className="settings-layout">
        <nav className="settings-nav"><button className={section === "profile" ? "active" : ""} onClick={() => setSection("profile")}><UserRound size={17} />Conta e perfil</button><button className={section === "minecraft" ? "active" : ""} onClick={() => setSection("minecraft")}><Gamepad2 size={17} />Minecraft</button><button className={section === "network" ? "active" : ""} onClick={() => setSection("network")}><Globe2 size={17} />Rede e fontes</button><button className={section === "appearance" ? "active" : ""} onClick={() => setSection("appearance")}><Image size={17} />Aparência</button><button className={section === "advanced" ? "active" : ""} onClick={() => setSection("advanced")}><Code2 size={17} />Avançado</button><button className={section === "help" ? "active" : ""} onClick={() => setSection("help")}><CircleHelp size={17} />Ajuda</button></nav>
        <div className="settings-content">
          {section === "profile" && <><section className="settings-group"><div className="settings-heading"><div><h2>Editar perfil offline</h2><p>Nome e skin locais usados quando o modo offline estiver ativo.</p></div><span className={`saved-state ${saved ? "" : "pending"}`}>{saved ? <Check size={15} /> : <MessageSquareText size={15} />}{saved ? "Salvo" : "Alterações pendentes"}</span></div>
            <div className="profile-editor"><div className="skin-preview large"><SkinFace skinDataUrl={skinDataUrl} label={draftName} large /></div><div className="grow"><label>Nome offline</label><div className="input-action"><input value={draftName} onChange={(event) => { setDraftName(event.target.value); setSaved(false); }} maxLength={16} /><button onClick={saveProfile}>Salvar</button></div><span className="field-hint">Entre 3 e 16 caracteres, usando letras, números ou _.</span><input ref={skinInput} className="visually-hidden" type="file" accept="image/png" onChange={(event) => void saveSkin(event.target.files?.[0])} /><div className="button-row"><button className="secondary-button" onClick={() => skinInput.current?.click()}><Image size={16} />Escolher skin</button><button className="text-button danger-text" onClick={() => void removeSkin()}><X size={16} />Remover skin</button></div><span className="skin-status">{skinStatus}</span></div></div>
          </section>
          <section className="settings-group"><div className="settings-heading"><div><h2>Conta Microsoft</h2><p>Login oficial com Xbox Live e Minecraft. Sua senha nunca passa pelo VEX.</p></div><span className={`saved-state ${microsoftAccount.logged_in ? "" : "pending"}`}>{microsoftAccount.logged_in ? <Check size={15} /> : <MessageSquareText size={15} />}{microsoftAccount.logged_in ? "Conectada" : "Não conectada"}</span></div>
            <div className="account-row"><span className="account-icon">{microsoftAccount.logged_in ? <ShieldCheck size={19} /> : <Box size={19} />}</span><span className="grow"><b>{microsoftAccount.logged_in ? microsoftAccount.username : "Nenhuma conta Microsoft"}</b><small>{microsoftAccount.logged_in ? (useOfflineProfile ? "Conta salva, perfil offline ativo" : "Conta Microsoft ativa em todas as instâncias") : "Entre para usar seu nome, licença e skin oficiais."}</small></span>{microsoftAccount.logged_in ? <><button className="secondary-button" disabled={!useOfflineProfile} onClick={onUseMicrosoft}>{useOfflineProfile ? "Usar conta" : "Em uso"}</button><button className="text-button danger-text" onClick={onMicrosoftLogout}>Sair</button></> : <button className="primary-button small" onClick={onMicrosoftLogin}><ShieldCheck size={15} />Entrar com Microsoft</button>}</div>
          </section>
          <section className="settings-group"><div className="settings-heading"><div><h2>Perfil offline</h2><p>Use o nome e a skin locais quando não quiser entrar com a Microsoft.</p></div><span className={`saved-state ${useOfflineProfile ? "" : "pending"}`}>{useOfflineProfile ? <Check size={15} /> : <UserRound size={15} />}{useOfflineProfile ? "Em uso" : "Disponível"}</span></div><div className="account-row"><span className="account-icon"><UserRound size={19} /></span><span className="grow"><b>{username}</b><small>{skinDataUrl ? "Skin offline personalizada salva" : "Skin clássica salva"}</small></span><button className="secondary-button" disabled={useOfflineProfile} onClick={onUseOffline}>{useOfflineProfile ? "Em uso" : "Usar offline"}</button></div></section>
          <button className="logout-button" onClick={() => void logout()}><LogOut size={17} />Redefinir perfil offline</button></>}
          {section === "minecraft" && <section className="settings-group"><div className="settings-heading"><div><h2>Pastas e armazenamento</h2><p>Downloads e instâncias permanecem no disco escolhido.</p></div></div><div className="setting-row"><span><b>Pasta do Minecraft</b><small>{gameDirectory}</small></span><div className="button-row inline"><button className="secondary-button compact" onClick={() => void invoke("open_path", { path: gameDirectory }).catch((error) => notify(String(error)))}><FolderOpen size={15} />Abrir</button><button className="secondary-button compact" onClick={() => void changeDirectory()}>Alterar</button></div></div><div className="setting-row"><span><b>Cache do launcher</b><small>{storageRoot} · downloads temporários</small></span><button className="secondary-button compact" onClick={() => void clearCache()}>Limpar</button></div><div className="setting-row"><span><b>Java detectado</b><small>{javaRuntimes[0] ? `Java ${javaRuntimes[0].major} · ${javaRuntimes[0].path}` : "Nenhum Java encontrado"}</small></span><button className="secondary-button compact" onClick={() => setSection("advanced")}>Gerenciar</button></div></section>}
          {section === "network" && <section className="settings-group"><div className="settings-heading"><div><h2>Rede e fontes</h2><p>Conteúdo é pesquisado e baixado diretamente de fontes conhecidas.</p></div></div><div className="setting-row"><span><b>Modrinth</b><small>Mods, modpacks, shaders e texturas</small></span><button className="secondary-button compact" onClick={() => void openUrl("https://modrinth.com")}>Abrir</button></div><div className="setting-row"><span><b>playit.gg</b><small>Túnel opcional para compartilhar servidores locais</small></span><button className="secondary-button compact" onClick={() => void openUrl("https://playit.gg")}>Abrir</button></div></section>}
          {section === "appearance" && <section className="settings-group"><div className="settings-heading"><div><h2>Aparência</h2><p>Interface escura, alto contraste e densidade ajustável.</p></div></div><div className="setting-row"><span><b>Modo compacto</b><small>Reduz espaços em listas longas.</small></span><button className="secondary-button compact" onClick={() => { setDense(!dense); document.body.classList.toggle("dense-ui", !dense); }}>{dense ? "Desativar" : "Ativar"}</button></div></section>}
          {section === "advanced" && <section className="settings-group"><div className="settings-heading"><div><h2>Java detectado</h2><p>O launcher seleciona automaticamente uma versão compatível.</p></div></div>{javaRuntimes.map((runtime) => <div className="setting-row" key={runtime.path}><span><b>Java {runtime.major}</b><small>{runtime.path}</small></span><button className="secondary-button compact" onClick={() => void invoke("open_path", { path: runtime.path }).catch((error) => notify(String(error)))}>Abrir</button></div>)}{!javaRuntimes.length && <div className="empty-state"><Code2 size={22} /><span>Nenhum Java encontrado.</span></div>}</section>}
          {section === "help" && <section className="settings-group"><div className="settings-heading"><div><h2>Ajuda</h2><p>Atalhos para documentação e suporte oficiais.</p></div></div><div className="setting-row"><span><b>Ajuda do Minecraft</b><small>Conta, instalação e solução de problemas</small></span><button className="secondary-button compact" onClick={() => void openUrl("https://help.minecraft.net")}>Abrir</button></div><div className="setting-row"><span><b>Pasta de logs</b><small>{storageRoot}\logs</small></span><button className="secondary-button compact" onClick={() => void invoke("open_path", { path: `${storageRoot}\\logs` }).catch((error) => notify(String(error)))}>Abrir</button></div></section>}
        </div>
      </div>
    </div>
  );
}

function App() {
  const [page, setPage] = useState<Page>("home");
  const [compact, setCompact] = useState(false);
  const [sidebarOpen, setSidebarOpen] = useState(false);
  const [toast, setToast] = useState("");
  const [bootProgress, setBootProgress] = useState(8);
  const [booting, setBooting] = useState(true);
  const [operationProgress, setOperationProgress] = useState<OperationProgress | null>(null);
  const [username, setUsername] = useState("Player");
  const [skinDataUrl, setSkinDataUrl] = useState<string>();
  const [microsoftAccount, setMicrosoftAccount] = useState<MicrosoftAccount>({ logged_in: false, active: false, username: "", uuid: "" });
  const [useOfflineProfile, setUseOfflineProfile] = useState(true);
  const [onboardingCompleted, setOnboardingCompleted] = useState(false);
  const [authBusy, setAuthBusy] = useState(false);
  const [appInstances, setAppInstances] = useState<Instance[]>([]);
  const [storageRoot, setStorageRoot] = useState("D:\\MineLauncher");
  const [gameDirectory, setGameDirectory] = useState("D:\\.minecraft");
  const [javaRuntimes, setJavaRuntimes] = useState<JavaRuntime[]>([]);
  const notify = (message: string, duration = 3200) => {
    setToast(message);
    window.setTimeout(() => setToast(""), duration);
  };
  const refreshInstances = async () => {
    const found = await invoke<BackendInstance[]>("list_installed_instances");
    const colors: Record<string, string> = { fabric: "#66899d", quilt: "#7b8797", forge: "#8b7d6b", neoforge: "#6d7f96", vanilla: "#579c82" };
    const mapped = await Promise.all(found.map(async (instance) => ({
      id: instance.id,
      name: instance.name,
      loader: instance.loader.charAt(0).toUpperCase() + instance.loader.slice(1),
      version: instance.mc_version,
      lastPlayed: instance.kind === "modpack" ? "Modpack instalado" : `${instance.size_mb} MB`,
      color: colors[instance.loader] ?? "#777b87",
      icon: initials(instance.name),
      iconUrl: instance.icon_path ? await invoke<string>("read_image_data_url", { path: instance.icon_path }).catch(() => undefined) : undefined,
      mods: instance.kind === "modpack" ? 1 : 0,
      versionId: instance.version_id,
      profileDir: instance.profile_dir,
      kind: instance.kind,
      sizeMb: instance.size_mb
    })));
    setAppInstances(mapped);
  };
  useEffect(() => {
    let mounted = true;
    let finishTimer = 0;
    const initialize = async () => {
      setBootProgress(18);
      const settings = await invoke<LauncherSettingsResult>("get_launcher_settings").catch(() => null);
      if (!mounted) return;
      if (settings) {
        setUsername(settings.offline_username);
        setStorageRoot(settings.storage_root);
        setGameDirectory(settings.game_directory);
        setUseOfflineProfile(settings.use_offline_profile);
        setOnboardingCompleted(settings.onboarding_completed);
        if (settings.offline_skin_path) {
          const skin = await invoke<string>("read_image_data_url", { path: settings.offline_skin_path }).catch(() => undefined);
          if (mounted) setSkinDataUrl(skin);
        }
      }
      const account = await invoke<MicrosoftAccount>("get_microsoft_account").catch(() => null);
      if (mounted && account) setMicrosoftAccount(account);
      setBootProgress(44);
      await refreshInstances().catch(() => undefined);
      if (!mounted) return;
      setBootProgress(72);
      const runtimes = await invoke<JavaRuntime[]>("detect_java_runtimes").catch(() => []);
      if (!mounted) return;
      setJavaRuntimes(runtimes);
      setBootProgress(100);
      finishTimer = window.setTimeout(() => mounted && setBooting(false), 320);
    };
    void initialize();
    const unlistenPromises = isTauri()
      ? [listen<OperationProgress>("operation-progress", ({ payload }) => {
          if (!mounted) return;
          setOperationProgress(payload);
          if (payload.done) {
            window.setTimeout(() => mounted && setOperationProgress((current) => current?.operation === payload.operation ? null : current), 1800);
          }
        }), listen<string>("microsoft-auth-code", async ({ payload }) => {
          if (!mounted) return;
          setAuthBusy(true);
          try {
            const account = await invoke<MicrosoftAccount>("complete_microsoft_login", { code: payload });
            if (!mounted) return;
            setMicrosoftAccount(account);
            setUseOfflineProfile(false);
            setOnboardingCompleted(true);
            notify(`Conta Microsoft ${account.username} conectada`, 5200);
          } catch (error) {
            notify(`Falha no login Microsoft: ${String(error)}`, 7200);
          } finally {
            if (mounted) setAuthBusy(false);
          }
        }), listen<string>("microsoft-auth-error", ({ payload }) => {
          if (!mounted) return;
          setAuthBusy(false);
          notify(payload, 6200);
        })]
      : [Promise.resolve(() => undefined)];
    return () => {
      mounted = false;
      window.clearTimeout(finishTimer);
      for (const unlistenPromise of unlistenPromises) void unlistenPromise.then((unlisten) => unlisten());
    };
  }, []);
  const saveProfile = async (newUsername: string) => {
    setUsername(newUsername);
    setUseOfflineProfile(true);
    setOnboardingCompleted(true);
    notify(`Perfil ${newUsername} salvo`, 2300);
    await invoke("save_offline_profile", { username: newUsername, skinPath: null }).catch(() => undefined);
  };
  const beginMicrosoftLogin = async () => {
    setAuthBusy(true);
    try {
      await invoke("begin_microsoft_login");
      notify("Conclua o login na janela oficial da Microsoft", 5200);
    } catch (error) {
      notify(`Não foi possível abrir o login Microsoft: ${String(error)}`, 7200);
    } finally {
      setAuthBusy(false);
    }
  };
  const chooseOffline = async () => {
    try {
      await invoke("choose_offline_mode");
      setUseOfflineProfile(true);
      setOnboardingCompleted(true);
      notify(`Perfil offline ${username} ativo`);
    } catch (error) {
      notify(String(error));
    }
  };
  const useMicrosoft = async () => {
    try {
      const account = await invoke<MicrosoftAccount>("use_microsoft_account");
      setMicrosoftAccount(account);
      setUseOfflineProfile(false);
      setOnboardingCompleted(true);
      notify(`Conta Microsoft ${account.username} ativa`);
    } catch (error) {
      notify(String(error));
    }
  };
  const logoutMicrosoft = async () => {
    if (!window.confirm("Sair da conta Microsoft salva no VEX?")) return;
    try {
      const account = await invoke<MicrosoftAccount>("logout_microsoft_account");
      setMicrosoftAccount(account);
      setUseOfflineProfile(true);
      notify("Conta Microsoft removida deste computador");
    } catch (error) {
      notify(String(error));
    }
  };
  const play = async (instance: Instance) => {
    if (!instance.versionId || !instance.profileDir) {
      notify("Esta instância não possui dados suficientes para iniciar", 2800);
      return;
    }
    notify(`Preparando ${instance.name}...`, 6500);
    try {
      await invoke("launch_instance", { versionId: instance.versionId, profileDir: instance.profileDir });
      notify(`${instance.name} iniciado`, 6500);
    } catch (error) {
      notify(`Falha: ${String(error)}`, 6500);
    }
  };
  const navigate = (next: Page) => { setPage(next); setSidebarOpen(false); };
  const activeUsername = !useOfflineProfile && microsoftAccount.logged_in ? microsoftAccount.username : username;
  const activeSkin = !useOfflineProfile && microsoftAccount.logged_in ? microsoftAccount.skin_url : skinDataUrl;
  const accountLabel = !useOfflineProfile && microsoftAccount.logged_in ? "Conta Microsoft" : "Perfil offline";
  return (
    <div className="app-window">
      {booting && <BootScreen progress={bootProgress} />}
      {!booting && !onboardingCompleted && <AccountChoiceModal onOffline={() => void chooseOffline()} onMicrosoft={() => void beginMicrosoftLogin()} busy={authBusy} />}
      <Topbar page={page} sidebarOpen={sidebarOpen} setSidebarOpen={setSidebarOpen} notify={notify} />
      <div className="app-body">
        <div className={`sidebar-mobile-wrap ${sidebarOpen ? "open" : ""}`}><Sidebar page={page} setPage={navigate} compact={compact} setCompact={setCompact} username={activeUsername} skinDataUrl={activeSkin} accountLabel={accountLabel} appInstances={appInstances} /></div>
        <main className="main-content">
          {page === "home" && <HomePage play={play} username={activeUsername} skinDataUrl={activeSkin} accountLabel={accountLabel} appInstances={appInstances} navigate={navigate} gameDirectory={gameDirectory} notify={notify} />}
          {page === "library" && <LibraryPage play={play} appInstances={appInstances} navigate={navigate} refreshInstances={refreshInstances} notify={notify} />}
          {page === "discover" && <DiscoverPage appInstances={appInstances} />}
          {page === "server" && <ServerPage notify={notify} storageRoot={storageRoot} />}
          {page === "logs" && <LogsPage storageRoot={storageRoot} />}
          {page === "settings" && <SettingsPage username={username} skinDataUrl={skinDataUrl} onSkinChanged={setSkinDataUrl} onSaveProfile={saveProfile} microsoftAccount={microsoftAccount} useOfflineProfile={useOfflineProfile} onMicrosoftLogin={() => void beginMicrosoftLogin()} onUseMicrosoft={() => void useMicrosoft()} onUseOffline={() => void chooseOffline()} onMicrosoftLogout={() => void logoutMicrosoft()} storageRoot={storageRoot} gameDirectory={gameDirectory} javaRuntimes={javaRuntimes} onGameDirectoryChanged={setGameDirectory} notify={notify} />}
        </main>
      </div>
      {operationProgress && <ProgressPanel progress={operationProgress} />}
      {toast && <div className="toast"><span className="toast-spinner" /><span><b>{toast}</b><small>Verificando arquivos e perfil</small></span><ChevronRight size={17} /></div>}
    </div>
  );
}

export default App;
