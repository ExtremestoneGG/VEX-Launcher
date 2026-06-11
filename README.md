# VEX Launcher

O VEX é um launcher de Minecraft gratuito e de código aberto, criado para tornar simples o caminho entre escolher uma versão e começar a jogar. Ele usa Tauri 2, React, TypeScript e Rust para manter uma interface moderna sem carregar um navegador completo junto do aplicativo.

O projeto nasceu de uma experiência de design desenvolvida com programação assistida por IA. A proposta é ser leve, direto e acessível tanto para quem possui uma conta Microsoft quanto para quem usa um perfil offline.

## VEX 0.6

- Pesquisa combinada no Modrinth e CurseForge, com filtros por fonte, versão, loader e tipo de conteúdo.
- Páginas próprias para mods, modpacks, shaders, texturas e plugins, com versões disponíveis e instalação compatível.
- Instâncias Vanilla, Fabric, Quilt, Forge e NeoForge.
- Instalação oficial de Forge e NeoForge, sem criar instâncias Vanilla disfarçadas.
- Instalação de modpacks Modrinth e CurseForge com verificação de integridade quando a fonte fornece hashes.
- Download automático do Java compatível pelo Eclipse Adoptium, isolado dentro dos dados do VEX.
- Perfil offline salvo, biblioteca de skins e login oficial Microsoft no Windows.
- Biblioteca de instâncias, clonagem, exclusão protegida, mundos, capturas, logs e conteúdo instalado.
- Servidor local Vanilla, Paper ou Fabric com console e guia para uso do playit.gg.
- Temas Escuro, AMOLED, Claro e Alto Contraste.
- Instalador por usuário, executável portátil autocontido e AppImage para Linux.
- Teste automático de abertura do AppImage para impedir a publicação de uma tela preta.
- Suporte opcional ao MangoHud no Linux.

## CurseForge

A API oficial do CurseForge exige uma chave gratuita. Por segurança, o VEX não publica nem envia uma chave privada dentro do código aberto.

1. Crie uma chave em [console.curseforge.com](https://console.curseforge.com/).
2. Abra **Configurações > Rede e fontes**.
3. Cole a chave e clique em **Conectar**.

No Windows, a chave é protegida para o usuário atual. No Linux, o arquivo local recebe permissão restrita ao próprio usuário. A chave nunca é mostrada novamente pela interface.

Alguns autores bloqueiam downloads por aplicativos externos. Nesses casos, o VEX informa a limitação e direciona para a página oficial do projeto.

## Privacidade e segurança

Mundos, skins, perfis, logs, tokens e configurações ficam apenas no computador do jogador e são ignorados pelo Git. Downloads automáticos usam HTTPS e são verificados por SHA-256, SHA-512 ou MD5 quando a fonte oficial fornece o hash.

Leia a política completa em [SECURITY.md](SECURITY.md).

## Desenvolvimento

Requisitos: Node.js, Rust e as dependências do Tauri para o sistema operacional.

```powershell
npm install
npm run tauri dev
```

Validação principal:

```powershell
npm run build
cargo check --manifest-path src-tauri/Cargo.toml
```

## Gerar versões

No Windows:

```powershell
.\build-portable.ps1
```

Esse processo gera o instalador e o executável portátil autocontido.

O AppImage Linux é gerado e testado automaticamente pelo GitHub Actions. Ele pode ser executado na maioria das distribuições modernas sem instalação:

```bash
chmod +x VEX-Launcher.AppImage
./VEX-Launcher.AppImage
```

## Limitações conhecidas

- O login Microsoft integrado ainda está disponível somente no Windows.
- Downloads bloqueados pelo autor no CurseForge precisam ser feitos na página oficial.
- O AppImage depende de um ambiente gráfico Linux compatível com WebKitGTK.

## Licença

MIT
