"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.deactivate = exports.activate = void 0;
const vscode = require("vscode");
const node_1 = require("vscode-languageclient/node");
let client;
function activate(context) {
    console.log('Activating ARK language server extension');
    let disposable = vscode.commands.registerCommand('ark.helloWorld', () => {
        vscode.window.showInformationMessage('Hello World from ark!');
    });
    context.subscriptions.push(disposable);
    let serverOptions = () => {
        // TODO: port needs to be configurable or discoverable
        console.log('Creating client socket transport');
        return (0, node_1.createClientSocketTransport)(9277).then(transport => {
            console.log('Waiting to connect to language server');
            return transport.onConnected().then((protocol) => {
                console.log('Connected, returning protocol transports');
                return {
                    reader: protocol[0],
                    writer: protocol[1]
                };
            });
        });
    };
    let clientOptions = {
        documentSelector: [{ scheme: 'file', language: 'R' }],
    };
    console.log('Creating language client');
    client = new node_1.LanguageClient('ark', 'ARK Language Server', serverOptions, clientOptions);
    client.start();
}
exports.activate = activate;
function deactivate() { }
exports.deactivate = deactivate;
//# sourceMappingURL=extension.js.map