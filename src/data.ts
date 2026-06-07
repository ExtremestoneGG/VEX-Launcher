export type Instance = {
  id: string;
  name: string;
  loader: string;
  version: string;
  lastPlayed: string;
  color: string;
  icon: string;
  iconUrl?: string;
  mods: number;
  versionId?: string;
  profileDir?: string;
  kind?: string;
  sizeMb?: number;
};

export type Project = {
  id: string;
  name: string;
  author: string;
  kind: string;
  description: string;
  versions: string[];
  downloads: string;
  color: string;
  icon: string;
  iconUrl?: string;
};

export const instances: Instance[] = [
  { id: "vanilla", name: "Sobrevivência", loader: "Vanilla", version: "1.21.5", lastPlayed: "Hoje, 12:42", color: "#8b5cf6", icon: "S", mods: 0 },
  { id: "create", name: "Create: Oficina", loader: "Fabric", version: "1.20.1", lastPlayed: "Ontem, 21:08", color: "#d79a63", icon: "C", mods: 84 },
  { id: "performance", name: "Performance+", loader: "Fabric", version: "1.21.4", lastPlayed: "2 jun, 18:30", color: "#57a889", icon: "P", mods: 23 },
  { id: "cobblemon", name: "Cobblemon", loader: "Fabric", version: "1.21.1", lastPlayed: "28 mai, 20:15", color: "#d45d70", icon: "O", mods: 117 }
];

export const projects: Project[] = [
  { id: "sodium", name: "Sodium", author: "jellysquid3", kind: "Mod", description: "Motor de renderização moderno que melhora muito o desempenho e a fluidez.", versions: ["1.21.5", "1.21.4", "1.21.1"], downloads: "142,8 mi", color: "#7c8fe8", icon: "Na" },
  { id: "iris", name: "Iris Shaders", author: "coderbot", kind: "Mod", description: "Suporte elegante a shaders, compatível com Sodium.", versions: ["1.21.5", "1.21.4", "1.20.1"], downloads: "96,4 mi", color: "#8572c9", icon: "Ir" },
  { id: "fresh", name: "Fresh Animations", author: "FreshLX", kind: "Textura", description: "Animações expressivas para criaturas mantendo o estilo original.", versions: ["1.21.x", "1.20.x"], downloads: "24,1 mi", color: "#ca785d", icon: "Fa" },
  { id: "complementary", name: "Complementary Reimagined", author: "EminGTR", kind: "Shader", description: "Shader equilibrado, bonito e cuidadosamente otimizado.", versions: ["1.21.x", "1.20.x", "1.19.x"], downloads: "31,7 mi", color: "#5791a6", icon: "Cr" },
  { id: "create-mod", name: "Create", author: "simibubi", kind: "Mod", description: "Engenharia e automação com mecanismos que parecem parte do Minecraft.", versions: ["1.20.1", "1.19.2"], downloads: "78,2 mi", color: "#b78a63", icon: "C" }
];
