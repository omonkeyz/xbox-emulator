#!/usr/bin/env python3
"""Stand-in for the MSVC librarian (`lib.exe`) on Linux.

Several build scripts run `lib.exe /DEF:x.def /OUT:x.lib` and link the `x.exp` it leaves
behind, which carries the DLL's export table: ordinals, aliases, forwarders. LLVM's
`llvm-lib` builds the import library but writes no `.exp`.

This script makes one. It turns every row of the `.def` into an `/EXPORT:` linker directive
and compiles them into an object file named `x.exp`. `lld-link` reads the directives from
that object exactly as it would read them from the command line, so the DLL gets the same
exports. Any other use is passed straight to `llvm-lib`.

The container puts this on PATH as `lib.exe`, which is the name the build scripts look for.
"""
import os
import subprocess
import sys
import tempfile


def exports(def_text):
    """Yield one /EXPORT directive per row of the EXPORTS section."""
    in_exports = False
    for line in def_text.splitlines():
        line = line.split(";", 1)[0].strip()
        if not line:
            continue
        words = line.split()
        if words[0].upper() in ("LIBRARY", "NAME"):
            in_exports = False
            continue
        if words[0].upper() == "EXPORTS":
            in_exports = True
            words = words[1:]
            if not words:
                continue
        if not in_exports:
            continue
        directive = words[0]  # `name`, `name=internal` or `name=other_dll.function`
        for word in words[1:]:
            if word.startswith("@"):
                directive += "," + word
            elif word.upper() in ("NONAME", "DATA", "PRIVATE"):
                directive += "," + word.upper()
            else:
                sys.exit(f"msvc-lib: unsupported export attribute {word!r} in: {line}")
        yield directive


def main():
    args = sys.argv[1:]
    opt = {a.split(":", 1)[0].upper(): a.split(":", 1)[1] for a in args if a[:1] in "/-" and ":" in a}
    def_path, out = opt.get("/DEF") or opt.get("-DEF"), opt.get("/OUT") or opt.get("-OUT")
    if not def_path or not out:
        os.execvp("llvm-lib", ["llvm-lib"] + args)

    subprocess.run(["llvm-lib"] + args, check=True)

    exp = os.path.splitext(out)[0] + ".exp"
    with open(def_path, encoding="utf-8") as f:
        rows = list(exports(f.read()))
    with tempfile.TemporaryDirectory() as tmp:
        src = os.path.join(tmp, "exports.c")
        with open(src, "w", encoding="utf-8") as f:
            for row in rows:
                f.write('#pragma comment(linker, "/EXPORT:%s")\n' % row.replace("\\", "\\\\").replace('"', '\\"'))
        subprocess.run(["clang-cl", "--target=x86_64-pc-windows-msvc", "/nologo", "/c", "/Fo" + exp, src], check=True)


if __name__ == "__main__":
    main()
