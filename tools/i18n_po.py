"""Genera `lang/<locale>/LC_MESSAGES/nuide.po` a partir de `lang/nuide.pot`.

Las traducciones se listan **en el mismo orden** que el `.pot` (que a su vez
sigue el orden de extracción de `tools/i18n_slint.py`). Al final se agregan dos
claves que usa el lado Rust (`t()`), no los `.slint`.

Uso:  python tools/i18n_po.py
"""

import pathlib
import re

ROOT = pathlib.Path(__file__).resolve().parent.parent
LANG = ROOT / "lang"
POT = LANG / "nuide.pot"

# Claves que sólo consume Rust (`i18n::t`).
RUST_KEYS = ["Idioma", "Se aplica al reiniciar"]

HEADERS = {
    "es": "Spanish",
    "en": "English",
    "pt-BR": "Portuguese (Brazil)",
}

# Traducciones EN, en el orden del .pot + RUST_KEYS.
EN = [
    "Close the dialog (Esc)",
    "Do nothing and close (Esc)",
    "Discard unsaved changes",
    "Save changes and continue",
    "Open the file in the editor (Enter)",
    "Discard local changes (git checkout --)",
    "Previous match (Shift+F3)",
    "Next match (F3)",
    "Match case",
    "Match whole word",
    "Regular expression",
    "Search only in the selection",
    "Show or hide the replace field",
    "Close the search (Esc)",
    "Replace this match (Enter)",
    "Replace all matches",
    "Zoom out the image (Ctrl+-)",
    "Zoom in the image (Ctrl++)",
    "Fit the image to the panel",
    "Show or hide generated files (build, target, node_modules)",
    "Refresh the file tree (F5)",
    "Create a new Git repository at the project root (git init)",
    "Stage all changes for the commit (git add -A)",
    "Unstage all files (git reset)",
    "Pull changes from the remote (git pull)",
    "View and switch branches (git branch)",
    "Commit the staged changes (git commit)",
    "FTP (21), FTPS (FTP + TLS, curl validates the certificate), or SFTP (SSH, 22)",
    "Site server, e.g. ftp.example.com",
    "Hosting FTP/SFTP user",
    "Stored in NuIde's config (%APPDATA%), indexed per project: it doesn't touch the repository",
    "Path on the server where the site goes, e.g. public_html or /www/my-site",
    "Which project folder is uploaded, e.g. build or dist (empty = whole project)",
    "Comma-separated list of extensions to upload",
    "SpASM script that decides what to upload. It prints lines \u201cUPLOAD <local> [<remote>]\u201d, \u201cSKIP <local>\u201d, \u201cLOG <text>\u201d. Without a script, the subfolder is uploaded, filtered by extension.",
    "Serves the project folder at http://localhost:<port>/ with CORS, to preview the site before uploading",
    "Local server port (1024-65535)",
    "Subfolder served, e.g. build or dist (empty = whole project)",
    "Access-Control-Allow-Origin. * for any origin; a specific domain if the page sends credentials",
    "Panel to browse the tables of a .db file in the project. Uses sqlite3.exe or Python, no extra libraries",
    "Pick a .db / .sqlite file; the panel opens as soon as there's a database",
    "Confirm and close (Enter)",
    "Close (Esc)",
    "Asks for confirmation and removes the provider from the global config",
    "Validates the fields and writes the union into opencode.json",
    "Regular expression (instead of literal text)",
    "Replaces all matches in the project (careful: cannot be undone)",
    "Close (Esc) \u2014 changes are already saved",
    "Find a .db / .sqlite file in the project",
    "Re-reads the database tables and rows",
    "NuIde is running with administrator privileges: the panel terminals are elevated",
    "Current Git branch - click the code panel to switch it",
    "Errors and warnings from the analysis of the open file (F6); F8 jumps to the next",
    "Language detected from the active file's extension",
    "Encoding used to read and save the file",
    "Cursor position (line, column) in the editor",
    "New shell session (F5 runs at the project root)",
    "Split the terminal into two views",
    "Close this terminal session",
    "Open project - click to choose another folder",
    "Open project and files, save, close tabs and exit",
    "Undo/redo, find and replace, go to line",
    "Where each panel is docked (Explorer, OpenCode, terminal, Git): click to move it",
    "Run the SpASM program with the active configuration (F5)",
    "Transpile and compile to a native binary (Ctrl+B), or compile and run (Ctrl+Shift+B)",
    "Launch OpenCode or Nuide-CLI in the agent panel",
    "SpASM syntax, shortcuts and about NuIde (F1)",
    "Minimize",
    "Maximize or restore",
    "Exit (asks if there are unsaved changes)",
    "Choose an existing project (Ctrl+O)",
    "Open a loose file with no project (Ctrl+Shift+O)",
    "Create a new file in the project (Ctrl+N)",
    # RUST_KEYS
    "Language",
    "Applies after restart",
]

# Traducciones pt-BR, mismo orden.
PT = [
    "Fechar o diálogo (Esc)",
    "Não fazer nada e fechar (Esc)",
    "Descartar as alterações não salvas",
    "Salvar as alterações e continuar",
    "Abrir o arquivo no editor (Enter)",
    "Descartar as alterações locais (git checkout --)",
    "Correspondência anterior (Shift+F3)",
    "Próxima correspondência (F3)",
    "Diferenciar maiúsculas/minúsculas",
    "Coincidir palavra inteira",
    "Expressão regular",
    "Buscar apenas na seleção",
    "Mostrar ou ocultar o campo de substituição",
    "Fechar a busca (Esc)",
    "Substituir esta correspondência (Enter)",
    "Substituir todas as correspondências",
    "Afastar a imagem (Ctrl+-)",
    "Aproximar a imagem (Ctrl++)",
    "Ajustar a imagem ao painel",
    "Mostrar ou ocultar arquivos gerados (build, target, node_modules)",
    "Atualizar a árvore de arquivos (F5)",
    "Criar um novo repositório Git na raiz do projeto (git init)",
    "Preparar todas as alterações para o commit (git add -A)",
    "Remover todos os arquivos do índice (git reset)",
    "Trazer alterações do remoto (git pull)",
    "Ver e trocar de branch (git branch)",
    "Confirmar as alterações preparadas (git commit)",
    "FTP (21), FTPS (FTP + TLS, o curl valida o certificado) ou SFTP (SSH, 22)",
    "Servidor do site, ex. ftp.meudominio.com",
    "Usuário de FTP/SFTP do hosting",
    "Fica na config do NuIde (%APPDATA%), indexada por projeto: não mexe no repositório",
    "Caminho no servidor onde o site fica, ex. public_html ou /www/meu-site",
    "Qual pasta do projeto é enviada, ex. build ou dist (vazio = projeto inteiro)",
    "Lista separada por vírgula de extensões enviadas",
    "Script SpASM que decide o que subir. Imprime linhas \u201cUPLOAD <local> [<remota>]\u201d, \u201cSKIP <local>\u201d, \u201cLOG <texto>\u201d. Sem script, sobe a subpasta filtrando por extensão.",
    "Serve a pasta do projeto em http://localhost:<porta>/ com CORS, para ver o site antes de subir",
    "Porta local do servidor (1024-65535)",
    "Subpasta servida, ex. build ou dist (vazio = projeto inteiro)",
    "Access-Control-Allow-Origin. * para qualquer origem; um domínio concreto se a página envia credenciais",
    "Painel para ver as tabelas de um arquivo .db do projeto. Usa sqlite3.exe ou Python, sem bibliotecas extras",
    "Escolha um arquivo .db / .sqlite; o painel abre assim que houver base",
    "Confirmar e fechar (Enter)",
    "Fechar (Esc)",
    "Pede confirmação e remove o provedor da config global",
    "Valida os campos e escreve a união em opencode.json",
    "Expressão regular (em vez de texto literal)",
    "Substitui todas as correspondências no projeto (cuidado: não pode ser desfeito)",
    "Fechar (Esc) \u2014 as alterações já foram salvas",
    "Buscar um arquivo .db / .sqlite do projeto",
    "Relê as tabelas e linhas da base",
    "O NuIde está rodando com privilégios de administrador: os terminais do painel são elevados",
    "Branch atual do Git - clique no painel de código para trocar",
    "Erros e avisos da análise do arquivo aberto (F6); F8 pula para o próximo",
    "Linguagem detectada pela extensão do arquivo ativo",
    "Codificação com que o arquivo é lido e salvo",
    "Posição do cursor (linha, coluna) no editor",
    "Nova sessão de shell (F5 roda na raiz do projeto)",
    "Dividir o terminal em duas vistas",
    "Fechar esta sessão de terminal",
    "Projeto aberto - clique para escolher outra pasta",
    "Abrir projeto e arquivos, salvar, fechar abas e sair",
    "Desfazer/refazer, buscar e substituir, ir para linha",
    "Onde cada painel está acoplado (explorador, OpenCode, terminal, Git): clique para mover",
    "Rodar o programa SpASM com a configuração ativa (F5)",
    "Transpilar e compilar para binário nativo (Ctrl+B) ou compilar e executar (Ctrl+Shift+B)",
    "Lançar OpenCode ou Nuide-CLI no painel do agente",
    "Sintaxe do SpASM, atalhos e sobre o NuIde (F1)",
    "Minimizar",
    "Maximizar ou restaurar",
    "Sair (pergunta se há alterações não salvas)",
    "Escolher um projeto existente (Ctrl+O)",
    "Abrir um arquivo avulso sem projeto (Ctrl+Shift+O)",
    "Criar um arquivo novo no projeto (Ctrl+N)",
    # RUST_KEYS
    "Idioma",
    "Aplica após reiniciar",
]


def parse_pot(text: str) -> list[str]:
    msgids = []
    cur = None
    for line in text.splitlines():
        m = re.match(r'^msgid "(.*)"$', line)
        if m:
            cur = m.group(1)
        m = re.match(r'^msgstr ""$', line)
        if m and cur is not None:
            if cur != "":
                msgids.append(cur)
            cur = None
    return msgids


def esc(s: str) -> str:
    return s.replace("\\", "\\\\").replace('"', '\\"')


def write_po(locale: str, msgids: list[str], strs: list[str]) -> None:
    out = [
        'msgid ""',
        'msgstr ""',
        '"Project-Id-Version: nuide\\n"',
        f'"Language: {locale}\\n"',
        '"Content-Type: text/plain; charset=UTF-8\\n"',
        '"Content-Transfer-Encoding: 8bit\\n"',
        "",
    ]
    for mid, tr in zip(msgids, strs):
        if tr and tr != mid:
            out.append(f'msgid "{esc(mid)}"')
            out.append(f'msgstr "{esc(tr)}"')
        else:
            out.append(f'msgid "{esc(mid)}"')
            out.append('msgstr ""')
        out.append("")
    path = LANG / locale / "LC_MESSAGES" / "nuide.po"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("\n".join(out), encoding="utf-8")
    print(f"{locale}: {sum(1 for t in strs if t)} traducciones -> {path.relative_to(ROOT)}")


def main() -> None:
    msgids = parse_pot(POT.read_text(encoding="utf-8"))
    assert len(EN) == len(msgids), f"EN: {len(EN)} vs {len(msgids)} claves"
    assert len(PT) == len(msgids), f"PT: {len(PT)} vs {len(msgids)} claves"
    write_po("es", msgids, msgids)  # identidad
    write_po("en", msgids, EN)
    write_po("pt-BR", msgids, PT)


if __name__ == "__main__":
    main()
