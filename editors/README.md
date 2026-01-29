# Ark LSP Editor Extensions

This directory contains editor configurations for using `ark-lsp`, a static R language server.

## Building ark-lsp

First, build the language server:

```bash
cd /path/to/ark
cargo build --release -p ark-lsp
```

The binary will be at `target/release/ark-lsp`.

## VS Code

### Installation

1. Install dependencies:
   ```bash
   cd editors/vscode
   npm install
   npm run compile
   ```

2. Either:
   - Add `ark-lsp` to your PATH, or
   - Set `ark-r.server.path` in VS Code settings to the full path

3. Install the extension:
   - Open VS Code
   - Run "Extensions: Install from VSIX..." (if packaged)
   - Or use "Developer: Install Extension from Location..." and select the `editors/vscode` folder

### Configuration

In VS Code settings (`settings.json`):

```json
{
  "ark-r.server.path": "/path/to/ark-lsp"
}
```

## Zed

### Installation

1. Copy the `editors/zed` folder to your Zed extensions directory:
   ```bash
   cp -r editors/zed ~/.config/zed/extensions/ark-r
   ```

2. Add `ark-lsp` to your PATH, or configure the path in Zed settings.

3. Add to your Zed `settings.json`:
   ```json
   {
     "lsp": {
       "ark-lsp": {
         "binary": {
           "path": "/path/to/ark-lsp",
           "arguments": ["--stdio"]
         }
       }
     },
     "languages": {
       "R": {
         "language_servers": ["ark-lsp"]
       }
     }
   }
   ```

## Features

The static LSP provides:

- **Syntax highlighting** (via tree-sitter)
- **Folding ranges** for functions, control structures, and braced expressions
- **Selection expansion** (Ctrl+Shift+→ / Cmd+Shift+→)
- **Document symbols** (Outline view)
- **Completions** for:
  - R keywords
  - Symbols defined in the current document
  - Package exports (when using `pkg::`)
  - Installed package names
- **Hover** information for identifiers
- **Signature help** when typing function calls
- **Go to definition** within the current document
- **Syntax error diagnostics**
- **Auto-indentation** on newlines

## Limitations

This is a *static* language server. Unlike the full Ark LSP in Positron, it does not:

- Provide completions from the R session's search path
- Show live variable values or types
- Support runtime introspection
- Provide help documentation from R's help system

For full R IDE features, use [Positron](https://github.com/posit-dev/positron).
