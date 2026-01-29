import * as path from 'path';
import * as vscode from 'vscode';
import {
    LanguageClient,
    LanguageClientOptions,
    ServerOptions,
} from 'vscode-languageclient/node';

let client: LanguageClient;

function getServerPath(context: vscode.ExtensionContext): string {
    const config = vscode.workspace.getConfiguration('ark-r');
    const configPath = config.get<string>('server.path');
    
    if (configPath) {
        return configPath;
    }

    // Use bundled binary
    const platform = process.platform;
    const binaryName = platform === 'win32' ? 'ark-lsp.exe' : 'ark-lsp';
    return path.join(context.extensionPath, 'bin', binaryName);
}

export function activate(context: vscode.ExtensionContext) {
    const serverPath = getServerPath(context);

    const serverOptions: ServerOptions = {
        command: serverPath,
        args: ['--stdio'],
    };

    const clientOptions: LanguageClientOptions = {
        documentSelector: [
            { scheme: 'file', language: 'r' },
            { scheme: 'untitled', language: 'r' },
        ],
        synchronize: {
            fileEvents: vscode.workspace.createFileSystemWatcher('**/*.{r,R,rmd,Rmd,qmd}'),
        },
    };

    client = new LanguageClient(
        'ark-r',
        'Ark R Language Server',
        serverOptions,
        clientOptions
    );

    client.start();
}

export function deactivate(): Thenable<void> | undefined {
    if (!client) {
        return undefined;
    }
    return client.stop();
}
