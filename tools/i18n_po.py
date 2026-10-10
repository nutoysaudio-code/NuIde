"""Sincroniza `lang/<locale>/LC_MESSAGES/nuide.po` con `lang/nuide.pot`.

Modelo *merge*: conserva las traducciones existentes de cada `.po` y, para las
claves nuevas del `.pot`, usa `EN` / `PT` (o las deja sin traducir → caen al
español). Así el catálogo puede crecer sin reescribir todo.

Uso:  python tools/i18n_po.py
"""

import pathlib
import re

ROOT = pathlib.Path(__file__).resolve().parent.parent
LANG = ROOT / "lang"
POT = LANG / "nuide.pot"

# Traducciones por `msgid`. Lo que no esté acá y no exista ya en el `.po` queda
# sin traducir (se muestra el español).
EN = {
    " (sin guardar)": " (unsaved)",
    " [activa]": " [active]",
    "{}  ({})": "{}  ({})",
    "  ({})": "  ({})",
    "Filtrar comandos o archivos por nombre": "Filter commands or files by name",
    "Archivo": "File",
    "Editar": "Edit",
    "Ver": "View",
    "Ejecutar": "Run",
    "Compilar": "Build",
    "Agente": "Agent",
    "Ayuda": "Help",
    "Clic para mover {} (actualmente: {})": "Click to move {} (currently: {})",
    "izquierda": "left",
    "derecha": "right",
    "abajo": "bottom",
    "oculto": "hidden",
    "Explorador": "Explorer",
    "OpenCode CLI": "OpenCode CLI",
    "Terminal": "Terminal",
    "Control de código": "Source control",
    "Nuevo archivo…": "New file…",
    "Abrir archivo…": "Open file…",
    "Abrir carpeta…": "Open folder…",
    "Guardar": "Save",
    "Guardar como…": "Save as…",
    "Cerrar pestaña": "Close tab",
    "Cerrar todas": "Close all",
    "Salir": "Exit",
    "Deshacer": "Undo",
    "Rehacer": "Redo",
    "Cortar": "Cut",
    "Copiar": "Copy",
    "Pegar": "Paste",
    "Seleccionar todo": "Select all",
    "Buscar": "Find",
    "Reemplazar": "Replace",
    "Ir a línea…": "Go to line…",
    "Lanzar OpenCode": "Launch OpenCode",
    "Lanzar Nuide-CLI (agente)": "Launch Nuide-CLI (agent)",
    "Reiniciar la terminal": "Restart the terminal",
    "Mostrar el panel del agente": "Show the agent panel",
    "Acerca de...": "About...",
    "Ayuda de sintaxis SpASM": "SpASM syntax help",
    "Preferencias y atajos": "Preferences and shortcuts",
    "Abrir carpeta de configuración": "Open configuration folder",
    "Ver repositorio": "View repository",
}

PT = {
    " (sin guardar)": " (não salvo)",
    " [activa]": " [ativa]",
    "{}  ({})": "{}  ({})",
    "  ({})": "  ({})",
    "Filtrar comandos o archivos por nombre": "Filtrar comandos ou arquivos por nome",
    "Archivo": "Arquivo",
    "Editar": "Editar",
    "Ver": "Ver",
    "Ejecutar": "Executar",
    "Compilar": "Compilar",
    "Agente": "Agente",
    "Ayuda": "Ajuda",
    "Clic para mover {} (actualmente: {})": "Clique para mover {} (atualmente: {})",
    "izquierda": "esquerda",
    "derecha": "direita",
    "abajo": "abaixo",
    "oculto": "oculto",
    "Explorador": "Explorador",
    "OpenCode CLI": "OpenCode CLI",
    "Terminal": "Terminal",
    "Control de código": "Controle de código",
    "Nuevo archivo…": "Novo arquivo…",
    "Abrir archivo…": "Abrir arquivo…",
    "Abrir carpeta…": "Abrir pasta…",
    "Guardar": "Salvar",
    "Guardar como…": "Salvar como…",
    "Cerrar pestaña": "Fechar aba",
    "Cerrar todas": "Fechar todas",
    "Salir": "Sair",
    "Deshacer": "Desfazer",
    "Rehacer": "Refazer",
    "Cortar": "Recortar",
    "Copiar": "Copiar",
    "Pegar": "Colar",
    "Seleccionar todo": "Selecionar tudo",
    "Buscar": "Buscar",
    "Reemplazar": "Substituir",
    "Ir a línea…": "Ir para linha…",
    "Lanzar OpenCode": "Iniciar OpenCode",
    "Lanzar Nuide-CLI (agente)": "Iniciar Nuide-CLI (agente)",
    "Reiniciar la terminal": "Reiniciar o terminal",
    "Mostrar el panel del agente": "Mostrar o painel do agente",
    "Acerca de...": "Sobre...",
    "Ayuda de sintaxis SpASM": "Ajuda de sintaxe SpASM",
    "Preferencias y atajos": "Preferências e atalhos",
    "Abrir carpeta de configuración": "Abrir pasta de configuração",
    "Ver repositorio": "Ver repositório",
}

OVERRIDES = {"en": EN, "pt-BR": PT}
HEADERS = {"es": "Spanish", "en": "English", "pt-BR": "Portuguese (Brazil)"}


def pot_keys(text: str) -> list[str]:
    return [m.group(1) for m in re.finditer(r'^msgid "((?:[^"\\]|\\.)*)"$', text, re.M) if m.group(1)]


def existing(path: pathlib.Path) -> dict[str, str]:
    if not path.exists():
        return {}
    out: dict[str, str] = {}
    mid = None
    for line in path.read_text(encoding="utf-8").splitlines():
        m = re.match(r'^msgid "((?:[^"\\]|\\.)*)"$', line)
        if m:
            mid = m.group(1)
            continue
        m = re.match(r'^msgstr "((?:[^"\\]|\\.)*)"$', line)
        if m and mid:
            if m.group(1):
                out[mid] = m.group(1)
            mid = None
    return out


def esc(s: str) -> str:
    return s.replace("\\", "\\\\").replace('"', '\\"')


def write_po(locale: str, keys: list[str], tr: dict[str, str]) -> None:
    out = [
        'msgid ""',
        'msgstr ""',
        '"Project-Id-Version: nuide\\n"',
        f'"Language: {locale}\\n"',
        '"Content-Type: text/plain; charset=UTF-8\\n"',
        '"Content-Transfer-Encoding: 8bit\\n"',
        "",
    ]
    translated = 0
    for k in keys:
        trs = tr.get(k, "")
        if trs:
            translated += 1
        out.append(f'msgid "{esc(k)}"')
        out.append(f'msgstr "{esc(trs)}"' if trs else 'msgstr ""')
        out.append("")
    path = LANG / locale / "LC_MESSAGES" / "nuide.po"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("\n".join(out), encoding="utf-8")
    print(f"{locale}: {translated}/{len(keys)} traducidas -> {path.relative_to(ROOT)}")


def main() -> None:
    keys = pot_keys(POT.read_text(encoding="utf-8"))
    unknown = (set(EN) | set(PT)) - set(keys)
    if unknown:
        print("AVISO: traducciones para claves que no están en el .pot:")
        for k in sorted(unknown):
            print("  -", repr(k))
    for locale in ("es", "en", "pt-BR"):
        tr = existing(LANG / locale / "LC_MESSAGES" / "nuide.po")
        tr.update(OVERRIDES.get(locale, {}))
        if locale == "es":
            tr = {}  # identidad: sin traducción, se muestra el español (msgid)
        write_po(locale, keys, tr)


if __name__ == "__main__":
    main()
