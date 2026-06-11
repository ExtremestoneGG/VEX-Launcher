# Política de segurança

## Dados locais

O VEX armazena perfis, skins, logs, instâncias, servidores, mundos, configurações e runtimes Java localmente. Esses dados não fazem parte do repositório e nunca devem ser enviados em commits, issues ou relatórios públicos.

O launcher não envia mundos, skins, nomes de usuário, logs ou arquivos de instâncias para serviços próprios. A rede é usada somente para autenticação solicitada pelo jogador e para baixar metadados, conteúdo e runtimes de fontes conhecidas.

## Credenciais

- No Windows, o token de renovação da conta Microsoft e a chave do CurseForge são protegidos para o usuário atual com DPAPI.
- No Linux, a chave do CurseForge é armazenada em um arquivo local acessível somente pelo próprio usuário.
- Senhas Microsoft são digitadas apenas na página oficial da Microsoft e nunca passam pelo VEX.
- Nenhuma chave, token ou dado pessoal deve ser incluído no código-fonte.

## Downloads automáticos

- Java: Eclipse Adoptium, verificado com SHA-256.
- Modrinth: arquivos oficiais, verificados com SHA-512 quando disponível.
- CurseForge: arquivos da CDN oficial, verificados com MD5 quando disponível.
- Forge e NeoForge: instaladores obtidos dos repositórios Maven oficiais.

O VEX restringe instalações automáticas às pastas configuradas do Minecraft, das instâncias e dos servidores.

## Relatar uma vulnerabilidade

Não publique vulnerabilidades que incluam tokens, caminhos pessoais, logs privados ou dados de jogadores em uma issue pública. Entre em contato de forma privada com o responsável pelo projeto e inclua somente os passos mínimos para reproduzir o problema.
