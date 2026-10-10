"""Envuelve los tooltips de `ui/*.slint` en `@tr(...)` y extrae el catálogo.

Es un paso de desarrollo (idempotente): deja los literales listos para que
`slint_build` los traduzca y regenera `lang/nuide.pot`. Sólo toca:

  - `TipContent { text: "<literal>" ; }`  →  `TipContent { text: @tr("<literal>") ; }`
  - `tip: "<literal>" ; }`                →  `tip: @tr("<literal>") ; }`

No toca valores que no sean un literal cerrado (concatenaciones, ternarios,
propiedades) — esos se resuelven en la Fase 2 (desde Rust con `t()`).

El orden de extracción es estable: por archivo (alfabético), primero los
`TipContent` y después los `tip:`; así las traducciones de `tools/i18n_po.py`
siguen alineadas.
"""

import pathlib
import re

ROOT = pathlib.Path(__file__).resolve().parent.parent
UI = ROOT / "ui"
LANG = ROOT / "lang"

TIP_TXT = re.compile(r'(TipContent\s*\{\s*text:\s*)"([^"]+)"(\s*[;}])')
TIP_PROP = re.compile(r'(\btip:\s*)"([^"]+)"(\s*[;}])')

# Ya envueltos (para re-extraer).
ATR_TXT = re.compile(r'TipContent\s*\{\s*text:\s*@tr\("((?:[^"\\]|\\.)*)"\)')
ATR_PROP = re.compile(r'\btip:\s*@tr\("((?:[^"\\]|\\.)*)"\)')

# Claves que consume Rust (`i18n::t`), no los `.slint`.
RUST_KEYS = ["Idioma", "Se aplica al reiniciar"]


def esc(s: str) -> str:
    return s.replace("\\", "\\\\").replace('"', '\\"')


def main() -> None:
    files = sorted(UI.glob("*.slint"))

    # 1) Envolver (idempotente).
    for f in files:
        text = f.read_text(encoding="utf-8")
        new = TIP_TXT.sub(lambda m: f'{m.group(1)}@tr("{m.group(2)}"){m.group(3)}', text)
        new = TIP_PROP.sub(lambda m: f'{m.group(1)}@tr("{m.group(2)}"){m.group(3)}', new)
        if new != text:
            f.write_text(new, encoding="utf-8")
            print(f"envuelto: {f.name}")

    # 2) Extraer en el mismo orden (TipContent, después tip:).
    msgs: list[str] = []
    seen: set[str] = set()

    def add(val: str) -> None:
        if val not in seen:
            seen.add(val)
            msgs.append(val)

    for f in files:
        text = f.read_text(encoding="utf-8")
        for m in ATR_TXT.finditer(text):
            add(m.group(1))
        for m in ATR_PROP.finditer(text):
            add(m.group(1))
    for k in RUST_KEYS:
        add(k)

    LANG.mkdir(parents=True, exist_ok=True)
    out = [
        'msgid ""',
        'msgstr ""',
        '"Project-Id-Version: nuide\\n"',
        '"Content-Type: text/plain; charset=UTF-8\\n"',
        '"Content-Transfer-Encoding: 8bit\\n"',
        "",
    ]
    for m in msgs:
        out.append(f'msgid "{esc(m)}"')
        out.append('msgstr ""')
        out.append("")
    (LANG / "nuide.pot").write_text("\n".join(out), encoding="utf-8")

    print(f"\n{len(msgs)} claves -> lang/nuide.pot")


if __name__ == "__main__":
    main()
