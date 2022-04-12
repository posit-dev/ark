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
        return (0, node_1.createClientSocketTransport)(9276).then(transport => {
            return transport.onConnected().then((protocol) => {
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
    client = new node_1.LanguageClient('ark', 'ARK Language Server', serverOptions, clientOptions);
    client.start();
}
exports.activate = activate;
function deactivate() { }
exports.deactivate = deactivate;
//# sourceMappingURL=extension.js.map