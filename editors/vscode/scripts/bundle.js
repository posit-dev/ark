const fs = require('fs');
const path = require('path');

const binDir = path.join(__dirname, '..', 'bin');
const srcBinary = path.join(__dirname, '..', '..', '..', 'target', 'release', 'ark-lsp');

if (!fs.existsSync(binDir)) {
    fs.mkdirSync(binDir, { recursive: true });
}

if (fs.existsSync(srcBinary)) {
    fs.copyFileSync(srcBinary, path.join(binDir, 'ark-lsp'));
    fs.chmodSync(path.join(binDir, 'ark-lsp'), 0o755);
    console.log('Bundled ark-lsp binary');
} else {
    console.error('ark-lsp binary not found. Run: cargo build --release -p ark-lsp');
    process.exit(1);
}
