#!/usr/bin/env python3
"""Check Git's exact spelling of paths and Rust/Cargo file references, on any OS.

Uses the index for names and working-tree contents for references. Checks all cfg
branches, including standalone spikes. No Cargo invocation or third-party modules.
Literal/raw strings and concat!(..., env!("CARGO_MANIFEST_DIR"), ...) are supported;
unresolvable include expressions fail closed instead of evading the guard.
"""
import argparse
from dataclasses import dataclass
from pathlib import Path, PurePosixPath
import posixpath
import re
import subprocess
import sys
import tomllib


@dataclass
class Token:
    value: str
    offset: int
    string: bool = False


def tokens(source):
    """Lex comments and string/character contents without recognizing code in them."""
    result = []
    i = 0
    while i < len(source):
        start = i
        if source[i].isspace():
            i += 1
        elif source.startswith('//', i):
            end = source.find('\n', i)
            i = len(source) if end < 0 else end
        elif source.startswith('/*', i):
            depth = 1
            i += 2
            while depth and i < len(source):
                if source.startswith('/*', i):
                    depth += 1
                    i += 2
                elif source.startswith('*/', i):
                    depth -= 1
                    i += 2
                else:
                    i += 1
            if depth:
                raise ValueError('unterminated comment')
        elif raw := re.match(r'(?:br|cr|r)(#*)"', source[i:]):
            begin = i + raw.end()
            end = source.find('"' + raw[1], begin)
            if end < 0:
                raise ValueError('unterminated raw string')
            result.append(Token(source[begin:end], start, True))
            i = end + 1 + len(raw[1])
        elif quoted := re.match(r'(?:b|c)?"', source[i:]):
            i += quoted.end()
            value = ''
            while i < len(source) and source[i] != '"':
                if source[i] != '\\':
                    value += source[i]
                    i += 1
                    continue
                i += 1
                if escape := re.match(r'u\{([0-9a-fA-F_]+)\}|x([0-9a-fA-F]{2})', source[i:]):
                    value += chr(int((escape[1] or escape[2]).replace('_', ''), 16))
                    i += escape.end()
                elif source[i] in '\r\n':
                    while i < len(source) and source[i].isspace():
                        i += 1
                else:
                    value += {'n': '\n', 'r': '\r', 't': '\t', '0': '\0'}.get(source[i], source[i])
                    i += 1
            if i == len(source):
                raise ValueError('unterminated string')
            result.append(Token(value, start, True))
            i += 1
        elif char := re.match(r"(?:b)?'(?:\\(?:u\{[0-9a-fA-F_]+\}|x[0-9a-fA-F]{2}|.)|[^'\\\n])'", source[i:]):
            i += char.end()
        elif ident := re.match(r'(?:r#)?[^\W\d]\w*', source[i:]):
            result.append(Token(ident[0].removeprefix('r#'), i))
            i += ident.end()
        else:
            result.append(Token(source[i], i))
            i += 1
    return result


def matching(ts, start):
    pairs = {'(': ')', '[': ']', '{': '}'}
    stack = []
    for i in range(start, len(ts)):
        if ts[i].string:
            continue
        value = ts[i].value
        if value in pairs:
            stack.append(pairs[value])
        elif value in pairs.values():
            if not stack or value != stack.pop():
                raise ValueError('unbalanced Rust delimiters')
            if not stack:
                return i
    raise ValueError('unclosed Rust delimiter')


class Check:
    def __init__(self, root, tracked):
        self.root = Path(root)
        self.tracked = set(tracked)
        self.errors = []
        self.references = 0
        self.folded = {}
        # Directory spelling must agree too, e.g. src/Foo/a.rs and src/foo/b.rs.
        for path in sorted(self.tracked):
            for prefix in [path, *map(str, PurePosixPath(path).parents)]:
                previous = self.folded.setdefault(prefix.casefold(), prefix)
                if previous != prefix:
                    self.errors.append(f'case collision: {previous!r} and {prefix!r}')
        self.manifests = {}
        self.roots = set()
        for path in sorted(self.tracked):
            if PurePosixPath(path).name == 'Cargo.toml':
                data = tomllib.loads((self.root / path).read_text())
                base = posixpath.dirname(path)
                self.manifests[base] = data
                for kind in ('lib', 'bin', 'test', 'bench', 'example'):
                    entries = data.get(kind, [])
                    for entry in [entries] if isinstance(entries, dict) else entries:
                        if 'path' in entry:
                            self.roots.add(posixpath.normpath(posixpath.join(base, entry['path'])))
                for rust in self.tracked:
                    relative = posixpath.relpath(rust, base)
                    if (relative in ('build.rs', 'src/lib.rs', 'src/main.rs')
                        or re.fullmatch(r'(tests|benches|examples|src/bin)/[^/]+\.rs', relative)
                        or re.fullmatch(r'(tests|benches|examples|src/bin)/[^/]+/main\.rs', relative)):
                        self.roots.add(rust)

    def require(self, origin, candidates, *, directory=False):
        self.references += 1
        candidates = [posixpath.normpath(c) for c in candidates]
        if any(c in self.tracked or (directory and c in self.folded.values()) for c in candidates):
            return
        hints = [self.folded[c.casefold()] for c in candidates if c.casefold() in self.folded]
        self.errors.append(f'{origin}: exact case is not tracked: {" or ".join(candidates)}'
                           + (f' (Git tracks {", ".join(hints)})' if hints else ''))

    def manifest_dir(self, source):
        return next((str(p) for p in PurePosixPath(source).parents if str(p) in self.manifests), '.')

    def expression(self, ts, source):
        ts = list(ts)
        if ts and ts[-1].value == ',' and not ts[-1].string:
            ts.pop()
        if len(ts) == 1 and ts[0].string:
            return ts[0].value
        values = [t.value for t in ts]
        if values == ['env', '!', '(', 'CARGO_MANIFEST_DIR', ')'] and ts[3].string:
            # Preserve spelling; never use case-insensitive Path.resolve here.
            return str(self.root / self.manifest_dir(source))
        if len(ts) >= 4 and values[:3] == ['concat', '!', '('] and values[-1] == ')':
            args = ts[3:-1]
            pieces = []
            start = i = 0
            while i < len(args):
                if not args[i].string and args[i].value in ('(', '[', '{'):
                    i = matching(args, i) + 1
                elif not args[i].string and args[i].value == ',':
                    pieces.append(self.expression(args[start:i], source))
                    start = i = i + 1
                else:
                    i += 1
            if start < len(args):
                pieces.append(self.expression(args[start:], source))
            return ''.join(pieces)
        raise ValueError('cannot statically resolve file reference; use a literal or literal concat!')

    def rust(self, path):
        source = (self.root / path).read_text()
        ts = tokens(source)
        parent = posixpath.dirname(path)
        module_dir = parent if path in self.roots or posixpath.basename(path) == 'mod.rs' else path[:-3]

        def origin(token):
            return f'{path}:{source.count(chr(10), 0, token.offset) + 1}'

        def includes(items):
            for i, token in enumerate(items[:-2]):
                if not token.string and token.value in ('include_str', 'include_bytes') and items[i+1].value == '!':
                    end = matching(items, i + 2)
                    try:
                        ref = self.expression(items[i+3:end], path)
                        if ref.startswith('/'):
                            target = posixpath.relpath(ref, str(self.root))
                        else:
                            target = posixpath.join(parent, ref)
                        self.require(origin(token), [target])
                    except ValueError as error:
                        self.errors.append(f'{origin(token)}: {error}')

        includes(ts)

        def modules(items, default_dir, attribute_dir):
            attrs = []
            i = 0
            while i < len(items):
                t = items[i]
                if not t.string and t.value == '#' and i+1 < len(items) and items[i+1].value == '[':
                    end = matching(items, i+1)
                    attrs.extend(items[i+2:end])
                    i = end + 1
                elif not t.string and t.value == 'mod' and i+2 < len(items) and items[i+2].value in (';', '{'):
                    name = items[i+1].value
                    paths = []
                    for j, attr in enumerate(attrs[:-2]):
                        if not attr.string and attr.value == 'path' and attrs[j+1].value == '=':
                            if not attrs[j+2].string:
                                raise ValueError(f'{origin(attr)}: nonliteral #[path]')
                            paths.append(posixpath.join(attribute_dir, attrs[j+2].value))
                    if items[i+2].value == ';':
                        if paths:
                            for ref in paths:
                                self.require(origin(t), [ref])
                        else:
                            self.require(origin(t), [posixpath.join(default_dir, name + '.rs'),
                                                     posixpath.join(default_dir, name, 'mod.rs')])
                        i += 3
                    else:
                        end = matching(items, i+2)
                        for directory in paths or [posixpath.join(default_dir, name)]:
                            modules(items[i+3:end], directory, directory)
                        i = end + 1
                    attrs = []
                elif not t.string and t.value in ('{', '(', '['):
                    end = matching(items, i)
                    # Blocks may contain modules. Parenthesized pub visibility
                    # preserves attributes; other item bodies consume them.
                    if t.value == '{':
                        modules(items[i+1:end], default_dir, attribute_dir)
                        attrs = []
                    i = end + 1
                else:
                    if not t.string and t.value == ';':
                        attrs = []
                    i += 1
        modules(ts, module_dir, parent)

    def cargo(self, base, data):
        for table in (data.get('package', {}), data.get('workspace', {}).get('package', {})):
            for key in ('readme', 'license-file'):
                value = table.get(key)
                if isinstance(value, str):
                    self.require(f'{base}/Cargo.toml ({key})', [posixpath.join(base, value)])

    def run(self):
        for base, data in self.manifests.items():
            self.cargo(base, data)
        for path in sorted(self.tracked):
            if path.endswith('.rs'):
                try:
                    self.rust(path)
                except (ValueError, OSError) as error:
                    self.errors.append(f'{path}: {error}')
        return sorted(set(self.errors))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, default=Path(__file__).resolve().parents[2])
    args = parser.parse_args()
    root = args.root.absolute()
    names = subprocess.check_output(['git', '-C', str(root), 'ls-files', '-z']).decode().split('\0')
    check = Check(root, [p for p in names if p])
    errors = check.run()
    for error in errors:
        print('ERROR: ' + error, file=sys.stderr)
    if errors:
        return 1
    print(f'PASS paths: {len(check.tracked)} tracked files; {check.references} Rust/Cargo references')
    return 0


if __name__ == '__main__':
    sys.exit(main())
