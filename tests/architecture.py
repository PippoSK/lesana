"""Check source-module boundaries in the single-crate architecture.

Expands use trees and resolves module-relative paths and import/dependency
aliases, including inline modules. Complements compilation: no macro expansion,
arbitrary type-alias resolution, or analysis of third-party implementations.
Generated code and include files must remain outside the inner layers.
"""
from dataclasses import dataclass
from pathlib import Path
import re
import sys
import tomllib

ROOT = Path(__file__).resolve().parent.parent
LAYERS = {"domain", "application", "adapters", "infrastructure"}
STDLIB = {"std", "core", "alloc"}
ALLOWED = {
    "domain": STDLIB | {"anyhow", "chrono", "regex", "serde"},
    "application": STDLIB | {
        "anyhow", "chrono", "futures_util", "regex", "serde", "serde_json",
        "tokio", "tracing", "url", "uuid",
    },
    "api": STDLIB | {
        "anyhow", "async_stream", "axum", "chrono", "chrono_tz", "futures_util", "regex",
        "serde", "serde_json", "sha2", "subtle", "tokio", "tokio_stream",
        "tower_http", "tracing", "url", "uuid",
    },
}
IO_NAMESPACES = {"fs", "io", "net", "process", "env", "os"}
FILE_MACROS = {"include", "include_str", "include_bytes", "env", "option_env"}
IDENT = re.compile(r"(?:r#)?[A-Za-z_][A-Za-z_0-9]*")
RAW_STRING = re.compile(r'(?:br|cr|r)(#*)"')
CHAR = re.compile(r"(?:b)?'(?:\\(?:u\{[0-9a-fA-F_]+\}|x[0-9a-fA-F]{2}|[^\n])|[^'\\\n])'")


@dataclass(frozen=True)
class Token:
    text: str
    line: int


def tokenize(source):
    """Ignore nested comments and literal contents, preserving line numbers."""
    result, i, line = [], 0, 1
    while i < len(source):
        start, start_line = i, line
        if source[i].isspace():
            i += 1
        elif source.startswith("//", i):
            end = source.find("\n", i)
            i = len(source) if end < 0 else end
        elif source.startswith("/*", i):
            i, depth = i + 2, 1
            while depth and i < len(source):
                if source.startswith("/*", i):
                    depth, i = depth + 1, i + 2
                elif source.startswith("*/", i):
                    depth, i = depth - 1, i + 2
                else:
                    i += 1
            if depth:
                raise ValueError("unterminated block comment")
        elif match := RAW_STRING.match(source, i):
            closing = '"' + match[1]
            end = source.find(closing, match.end())
            if end < 0:
                raise ValueError("unterminated raw string")
            i = end + len(closing)
            result.append(Token("<literal>", start_line))
        elif source[i] == '"' or source[i:i + 2] in {'b"', 'c"'}:
            i += 1 if source[i] == '"' else 2
            while i < len(source):
                if source[i] == "\\":
                    i += 2
                elif source[i] == '"':
                    i += 1
                    break
                else:
                    i += 1
            else:
                raise ValueError("unterminated string")
            result.append(Token("<literal>", start_line))
        elif match := CHAR.match(source, i):
            i = match.end()
            result.append(Token("<literal>", start_line))
        elif match := IDENT.match(source, i):
            i = match.end()
            result.append(Token(match[0].removeprefix("r#"), start_line))
        else:
            value = "::" if source.startswith("::", i) else source[i]
            i += len(value)
            result.append(Token(value, start_line))
        line += source[start:i].count("\n")
    return result


def use_paths(tokens):
    """Expand nested grouped imports into (path, alias) leaves."""
    values, output = [token.text for token in tokens], []

    def tree(i, prefix=()):
        path = list(prefix)
        if i < len(values) and values[i] == "::":
            i += 1
        while i < len(values) and IDENT.fullmatch(values[i]):
            path.append(values[i])
            i += 1
            if i < len(values) and values[i] == "::":
                i += 1
            else:
                break
        if i < len(values) and values[i] == "{":
            i += 1
            while i < len(values) and values[i] != "}":
                i = tree(i, tuple(path))
                if i < len(values) and values[i] == ",":
                    i += 1
                elif i >= len(values) or values[i] != "}":
                    raise ValueError("unrecognized grouped import")
            if i >= len(values):
                raise ValueError("unterminated grouped import")
            return i + 1
        if i < len(values) and values[i] == "*":
            path.append("*")
            i += 1
        if len(path) > 1 and path[-1] == "self":
            path.pop()
        if not path:
            raise ValueError("empty import")
        alias = path[-1]
        if i < len(values) and values[i] == "as":
            i += 1
            if i >= len(values) or not IDENT.fullmatch(values[i]):
                raise ValueError("invalid import alias")
            alias, i = values[i], i + 1
        output.append((tuple(path), alias))
        return i

    if tree(0) != len(values):
        raise ValueError("unrecognized import")
    return output


def module_scopes(tokens, module):
    scopes, stack, current = [], [], module
    for i, token in enumerate(tokens):
        scopes.append(current)
        if token.text == "{":
            stack.append(current)
            if i >= 2 and tokens[i - 2].text == "mod":
                current = (*current, tokens[i - 1].text)
        elif token.text == "}" and stack:
            current = stack.pop()
    return scopes


def inspect_source(source, module, dependencies=None, crate_name="lesana"):
    """Return (line, problem) pairs for e.g. ('domain', 'evidence')."""
    dependencies = dependencies or {}
    known_dependencies = set(dependencies) | set(dependencies.values())
    tokens = tokenize(source)
    scopes = module_scopes(tokens, module)
    imports, aliases, skipped, errors, children = [], {}, set(), [], set()
    values = [token.text for token in tokens]
    for i, token in enumerate(tokens):
        if token.text == "mod" and i + 2 < len(tokens) and values[i + 2] in {";", "{"}:
            children.add((scopes[i], values[i + 1]))
        if token.text == "use":
            end = next((j for j in range(i + 1, len(tokens)) if values[j] == ";"), len(tokens))
            skipped.update(range(i, end + 1))
            try:
                paths = use_paths(tokens[i + 1:end])
            except ValueError as error:
                errors.append((token.line, str(error)))
                continue
            for path, alias in paths:
                imports.append((path, scopes[i], token.line))
                if alias not in {"*", "_"}:
                    aliases[(scopes[i], alias)] = path
        elif values[i:i + 2] == ["extern", "crate"] and i + 2 < len(tokens):
            path = (values[i + 2],)
            alias = values[i + 4] if values[i + 3:i + 4] == ["as"] else path[0]
            imports.append((path, scopes[i], token.line))
            aliases[(scopes[i], alias)] = path

    def resolve(path, scope, seen=frozenset()):
        first, *rest = path
        if first in {"crate", crate_name}:
            return ("crate", *rest)
        if first in {"self", "super"}:
            base, parts = list(scope), list(path)
            if parts[0] == "self":
                parts.pop(0)
            while parts and parts[0] == "super":
                parts.pop(0)
                if base:
                    base.pop()
            return ("crate", *base, *parts)
        for depth in range(len(scope), -1, -1):
            key = (scope[:depth], first)
            if key in aliases and key not in seen:
                target = aliases[key]
                if target != (first,):
                    return (*resolve(target, scope[:depth], seen | {key}), *rest)
            if key in children:
                return ("crate", *scope[:depth], *path)
        return (dependencies.get(first, first), *rest)

    layer = module[0] if module else "root"
    if module[:2] == ("adapters", "api"):
        layer = "api"

    def check(path, scope, line, imported=False):
        resolved = resolve(path, scope)
        first = resolved[0]
        if first == "crate" and len(resolved) > 1:
            target = resolved[1:]
            permitted = (
                layer == "domain" and target[0] == "domain"
                or layer == "application" and target[0] in {"domain", "application"}
                or layer == "api" and (target[0] in {"domain", "application"} or target[:2] == ("adapters", "api"))
                or module[:2] == ("adapters", "spi") and (target[0] in {"domain", "application"} or target[:2] == ("adapters", "spi"))
                or layer in {"root", "infrastructure"}
                or module == ("adapters",)
            )
            if not permitted:
                errors.append((line, f"outward dependency {'::'.join(resolved)}"))
        elif layer in ALLOWED and (imported or first in known_dependencies or first in STDLIB):
            if first not in ALLOWED[layer]:
                errors.append((line, f"unapproved {layer} dependency {first}"))
        if layer in {"domain", "application"}:
            if first in {"std", "tokio"} and len(resolved) > 1 and resolved[1] in IO_NAMESPACES:
                errors.append((line, f"I/O belongs in an adapter: {'::'.join(resolved)}"))
            if imported and resolved[-1] == "*" and first != "crate":
                errors.append((line, "external wildcard imports hide dependency boundaries"))

    for path, scope, line in imports:
        check(path, scope, line, imported=True)
    i = 0
    while i < len(tokens):
        if i in skipped or not IDENT.fullmatch(values[i]):
            i += 1
            continue
        start, path = i, [values[i]]
        while i + 2 < len(tokens) and values[i + 1] == "::" and IDENT.fullmatch(values[i + 2]):
            path.append(values[i + 2])
            i += 2
        if len(path) > 1:
            check(tuple(path), scopes[start], tokens[start].line)
        if layer in {"domain", "application"} and i + 1 < len(tokens) and values[i + 1] == "!" and resolve(tuple(path), scopes[start])[-1] in FILE_MACROS:
            errors.append((tokens[start].line, f"{path[-1]}! belongs outside the inner layers"))
        i += 1
    for i in range(len(values) - 2):
        if values[i:i + 3] == ["#", "[", "path"]:
            errors.append((tokens[i].line, "#[path] can bypass the layer directory; use ordinary modules"))
    return sorted(set(errors))


def check_project(root):
    problems = []
    required = [
        "src/domain", "src/application/repositories", "src/application/usecases",
        "src/application/mappers", "src/application/utils", "src/adapters/api",
        "src/adapters/spi/db", "src/adapters/spi/http", "src/infrastructure",
    ]
    for directory in required:
        if not (root / directory).is_dir():
            problems.append(f"Missing reference layer directory: {directory}")
    for old in ("crates", "src/presentation"):
        if (root / old).exists():
            problems.append(f"Obsolete layer directory remains: {old}")
    manifest = tomllib.loads((root / "Cargo.toml").read_text())
    dependencies = {}
    for table in [manifest, *manifest.get("target", {}).values()]:
        for kind in ("dependencies", "dev-dependencies", "build-dependencies"):
            for alias, specification in table.get(kind, {}).items():
                name = specification.get("package", alias) if isinstance(specification, dict) else alias
                dependencies[alias.replace("-", "_")] = name.replace("-", "_")
    crate_name = manifest["package"]["name"].replace("-", "_")
    for source in sorted((root / "src").rglob("*.rs")):
        relative = source.relative_to(root / "src").with_suffix("")
        module = relative.parts[:-1] if relative.name == "mod" or relative.parts in {("lib",), ("main",)} else relative.parts
        try:
            for line, problem in inspect_source(source.read_text(), module, dependencies, crate_name):
                problems.append(f"{source.relative_to(root)}:{line}: {problem}")
        except ValueError as error:
            problems.append(f"{source.relative_to(root)}: cannot inspect source: {error}")
    expected_lib = {("pub", "mod", layer, ";") for layer in LAYERS}
    lib = [t.text for t in tokenize((root / "src/lib.rs").read_text())]
    if len(lib) != 16 or {tuple(lib[i:i + 4]) for i in range(0, len(lib), 4)} != expected_lib:
        problems.append("src/lib.rs must only expose the four architecture layers")
    main = [t.text for t in tokenize((root / "src/main.rs").read_text())]
    expected_main = [t.text for t in tokenize("#[tokio::main] async fn main() -> anyhow::Result<()> { lesana::infrastructure::run().await }")]
    expected_main = [crate_name if value == "lesana" else value for value in expected_main]
    if main != expected_main:
        problems.append("src/main.rs must delegate only to infrastructure::run().await")
    return problems


if __name__ == "__main__":
    failures = check_project(ROOT)
    if failures:
        print("\n".join(failures), file=sys.stderr)
        raise SystemExit(1)
    print("Architecture OK: domain <- application <- API/SPI adapters <- infrastructure.")
