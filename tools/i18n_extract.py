"""Extrae las claves de traducción a `lang/nuide.pot`.

Fuentes de claves:
  - `.slint`: todo `@tr("...")` (los tooltips se envuelven solos, ver abajo);
  - Rust: todo `i18n::t("...")` en `src/**/*.rs`;
  - claves de UI compuestas por Rust.

Modos:
  (sin flags)  envuelve los tooltips crudos de `ui/*.slint` en `@tr(...)` y
               reescribe `lang/nuide.pot`.
  --check      no modifica nada: falla (exit 1) si el `.pot` no está al día o si
               quedó algún tooltip sin envolver. Pensado para la CI.

El orden de las claves es estable: `.slint` (por archivo, alfabético, en orden de
aparición) y después Rust (por archivo, alfabético).
"""

import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
UI = ROOT / "ui"
SRC = ROOT / "src"
LANG = ROOT / "lang"
POT = LANG / "nuide.pot"

# Tooltips crudos (texto literal) → a envolver.
TIP_TXT = re.compile(r'(TipContent\s*\{\s*text:\s*)"([^"]+)"(\s*[;}])')
TIP_PROP = re.compile(r'(\btip:\s*)"([^"]+)"(\s*[;}])')

# Ya envueltos / en general.
ATR = re.compile(r'@tr\(\s*"((?:[^"\\]|\\.)*)"')
RUST_T = re.compile(r'i18n::t\(\s*"((?:[^"\\]|\\.)*)"')


def esc(s: str) -> str:
    return s.replace("\\", "\\\\").replace('"', '\\"')


def wrap_tooltips() -> bool:
    changed = False
    for f in sorted(UI.glob("*.slint")):
        text = f.read_text(encoding="utf-8")
        new = TIP_TXT.sub(lambda m: f'{m.group(1)}@tr("{m.group(2)}"){m.group(3)}', text)
        new = TIP_PROP.sub(lambda m: f'{m.group(1)}@tr("{m.group(2)}"){m.group(3)}', new)
        if new != text:
            f.write_text(new, encoding="utf-8")
            print(f"envuelto: {f.name}")
            changed = True
    return changed


def unwrapped_tooltips() -> list[str]:
    hits = []
    for f in sorted(UI.glob("*.slint")):
        text = f.read_text(encoding="utf-8")
        for m in list(TIP_TXT.finditer(text)) + list(TIP_PROP.finditer(text)):
            hits.append(f"{f.name}: {m.group(2)}")
    return hits


def collect() -> list[str]:
    keys: list[str] = []
    seen: set[str] = set()

    def add(k: str) -> None:
        if k not in seen:
            seen.add(k)
            keys.append(k)

    for f in sorted(UI.glob("*.slint")):
        for m in ATR.finditer(f.read_text(encoding="utf-8")):
            add(m.group(1))
    for f in sorted(SRC.rglob("*.rs")):
        for m in RUST_T.finditer(f.read_text(encoding="utf-8")):
            add(m.group(1))
    return keys


def render_pot(keys: list[str]) -> str:
    out = [
        'msgid ""',
        'msgstr ""',
        '"Project-Id-Version: nuide\\n"',
        '"Content-Type: text/plain; charset=UTF-8\\n"',
        '"Content-Transfer-Encoding: 8bit\\n"',
        "",
    ]
    for k in keys:
        out.append(f'msgid "{esc(k)}"')
        out.append('msgstr ""')
        out.append("")
    return "\n".join(out)


def pot_keys(text: str) -> list[str]:
    return [m.group(1) for m in re.finditer(r'^msgid "((?:[^"\\]|\\.)*)"$', text, re.M) if m.group(1)]


def main() -> int:
    check = "--check" in sys.argv
    if check:
        crudo = unwrapped_tooltips()
        if crudo:
            print("Hay tooltips sin envolver en @tr (corré tools/i18n_extract.py):")
            for h in crudo:
                print("  -", h)
            return 1
        actual = collect()
        if not POT.exists() or pot_keys(POT.read_text(encoding="utf-8")) != actual:
            print("lang/nuide.pot está desactualizado (corré tools/i18n_extract.py).")
            return 1
        print(f"i18n OK: {len(actual)} claves al día.")
        return 0

    wrap_tooltips()
    keys = collect()
    LANG.mkdir(parents=True, exist_ok=True)
    POT.write_text(render_pot(keys), encoding="utf-8")
    print(f"\n{len(keys)} claves -> lang/nuide.pot")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
