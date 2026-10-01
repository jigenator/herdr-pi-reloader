import { randomUUID } from "node:crypto";
import { chmodSync, lstatSync, mkdirSync } from "node:fs";
import { createServer, type Server, type Socket } from "node:net";
import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";

// No editor writes or terminal input. Drafts stay in place, including cursor position.
export default function (pi: ExtensionAPI) {
	const command = "herdr-pi-reloader-control";
	const generation = randomUUID();
	let server: Server | undefined;
	let closing = false;
	let pending: { socket: Socket; request: any; token: string } | undefined;

	function state(ctx: ExtensionContext) {
		if (closing || !ctx.isIdle() || ctx.hasPendingMessages()) return "busy";
		return ctx.ui.getEditorText() === "" ? "ready" : "draft";
	}

	function reply(socket: Socket, ctx: ExtensionContext, status: string, error?: string) {
		socket.end(JSON.stringify({
			protocol: 1, pid: process.pid, pane_id: process.env.HERDR_PANE_ID,
			server: process.env.HERDR_SOCKET_PATH,
			session_path: ctx.sessionManager.getSessionFile() ?? null,
			generation, status, error,
		}) + "\n");
	}

	function validate(request: any, ctx: ExtensionContext) {
		if (!request || request.protocol !== 1 || request.pid !== process.pid ||
			request.pane_id !== process.env.HERDR_PANE_ID ||
			request.server !== process.env.HERDR_SOCKET_PATH ||
			request.session_path !== (ctx.sessionManager.getSessionFile() ?? null) ||
			!["probe", "reload", "quit"].includes(request.action) ||
			(request.action !== "probe" && request.generation !== generation)) {
			throw new Error("Guard identity/session/runtime mismatch; no action taken");
		}
	}

	// Fail closed if a host version ever routes the internal command as ordinary input.
	pi.on("input", (event) => {
		if (event.text === `/${command}` || event.text.startsWith(`/${command} `)) return { action: "handled" };
	});

	// Dispatch a registered command to obtain ExtensionCommandContext.reload().
	// Explicit command expansion is required; this never enters the editor.
	pi.registerCommand(command, {
		description: "Internal draft-safe control for Herdr Pi Reloader",
		handler: async (token, ctx) => {
			if (!pending || token !== pending.token) return;
			const { socket, request } = pending;
			pending = undefined;
			if (socket.destroyed || socket.writableEnded) return;
			try {
				validate(request, ctx);
				const status = state(ctx); // Recheck immediately before the native action.
				if (status !== "ready") { reply(socket, ctx, status); return; }
				closing = true;
				reply(socket, ctx, "accepted");
				// No await between the final draft check and invoking Pi's native API.
				if (request.action === "quit") { ctx.shutdown(); return; }
				await ctx.reload();
				return; // The old runtime/context must not be used after reload.
			} catch {
				// An accepted action is verified by the caller (process exit/new runtime),
				// not assumed successful. Never fall back to typing a command.
				socket.destroy();
			}
		},
	});

	pi.on("session_start", async (_event, ctx) => {
		if (ctx.mode !== "tui" || !process.env.HERDR_SOCKET_PATH ||
			!process.env.HERDR_PANE_ID || !process.getuid) return;
		const uid = process.getuid();
		const directory = `/tmp/herdr-pi-reloader-${uid}`;
		const path = `${directory}/${process.pid}.sock`;
		try {
			mkdirSync(directory, { recursive: true, mode: 0o700 });
			const directoryInfo = lstatSync(directory);
			if (!directoryInfo.isDirectory() || directoryInfo.uid !== uid || (directoryInfo.mode & 0o077)) {
				throw new Error("Guard directory must be owned by this user and private (0700)");
			}
			closing = false;
			server = createServer((socket) => {
				socket.setEncoding("utf8");
				socket.setTimeout(3000, () => socket.destroy());
				socket.on("error", () => socket.destroy());
				let input = "";
				let handled = false;
				socket.on("data", (data) => {
					if (handled) return;
					input += data;
					if (Buffer.byteLength(input) > 4096) { socket.destroy(); return; }
					if (!input.includes("\n")) return;
					handled = true;
					try {
						const request = JSON.parse(input);
						validate(request, ctx);
						const status = pending ? "busy" : state(ctx);
						if (request.action === "probe" || status !== "ready") {
							reply(socket, ctx, status); return;
						}
						if (!pi.getCommands().some((item) => item.name === command && item.source === "extension")) {
							throw new Error("Guard command unavailable; no action taken");
						}
						const token = randomUUID();
						pending = { socket, request, token };
						pi.sendUserMessage(`/${command} ${token}`, { expandPromptTemplates: true });
					} catch (error) {
						if (pending?.socket === socket) pending = undefined;
						try { reply(socket, ctx, "error", String(error)); }
						catch { socket.destroy(); }
					}
				});
				socket.on("close", () => { if (pending?.socket === socket) pending = undefined; });
			});
			await new Promise<void>((resolve, reject) => {
				server!.once("error", reject);
				// Never unlink an existing endpoint: it may belong to another loaded copy.
				server!.listen(path, () => {
					try { chmodSync(path, 0o600); resolve(); }
					catch (error) { server!.close(); reject(error); }
				});
			});
		} catch (error) {
			ctx.ui.notify(`Pi Reloader guard unavailable: ${error}`, "warning");
		}
	});

	pi.on("session_shutdown", async () => {
		closing = true;
		pending?.socket.destroy();
		pending = undefined;
		if (server?.listening) await new Promise<void>((resolve) => server!.close(() => resolve()));
		server = undefined;
	});
}
