// Envoltorio mínimo: toda la lógica vive en el binario `huella lsp`.
import * as vscode from "vscode";
import { LanguageClient, LanguageClientOptions, ServerOptions } from "vscode-languageclient/node";

const LANGUAGES = ["typescript", "typescriptreact", "javascript", "javascriptreact", "python", "rust"];

let client: LanguageClient | undefined;

export async function activate(context: vscode.ExtensionContext): Promise<void> {
  context.subscriptions.push(
    vscode.commands.registerCommand("huella.restart", restart),
    vscode.workspace.onDidChangeConfiguration((event) => {
      if (event.affectsConfiguration("huella")) {
        void restart();
      }
    }),
  );
  await start();
}

export async function deactivate(): Promise<void> {
  await client?.stop();
}

async function start(): Promise<void> {
  const config = vscode.workspace.getConfiguration("huella");
  const args: string[] = [];
  const database = config.get<string>("databasePath", "").trim();
  if (database) {
    args.push("--db", database);
  }
  args.push("lsp", "--index-every", String(Math.max(0, Math.floor(config.get<number>("indexEverySeconds", 30)))));

  const command = config.get<string>("serverPath", "huella").trim() || "huella";
  const serverOptions: ServerOptions = { command, args };
  const clientOptions: LanguageClientOptions = {
    documentSelector: LANGUAGES.map((language) => ({ scheme: "file", language })),
  };
  client = new LanguageClient("huella", "Huella", serverOptions, clientOptions);
  try {
    await client.start();
  } catch (error) {
    client = undefined;
    const message = error instanceof Error ? error.message : String(error);
    void vscode.window.showErrorMessage(
      `Huella no pudo arrancar «${command}»: ${message}. Instálalo con «cargo install --path crates/huella-cli» o configura huella.serverPath.`,
    );
  }
}

async function restart(): Promise<void> {
  await client?.stop();
  client = undefined;
  await start();
}
