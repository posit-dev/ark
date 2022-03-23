import * as vscode from 'vscode';

export function activate(context: vscode.ExtensionContext) {

	console.log('Activating ARK language server extension');

	let disposable = vscode.commands.registerCommand('ark.helloWorld', () => {
		// The code you place here will be executed every time your command is executed
		// Display a message box to the user
		vscode.window.showInformationMessage('Hello World from ark!');
	});

	context.subscriptions.push(disposable);
}

export function deactivate() { }
