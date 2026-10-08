import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

/** Logo asset names to try in order, and a generic icon if none exists. */
export interface Brand {
  keys?: string[];
  icon?: string;
}

export interface AskOption {
  label: string;
  description?: string;
}

export type AgentEvent =
  | { kind: "tool_start"; id: string; name: string; label: string; detail?: string | null; brand?: Brand }
  | { kind: "tool_result"; id: string; name: string; ok: boolean; error?: string | null }
  | { kind: "progress"; text: string }
  | { kind: "ask_user"; id: string; question: string; options: AskOption[] }
  | { kind: "approval_request"; id: string; name: string; label: string; args: Record<string, unknown> };

/** Answers a question or approval card the agent is waiting on. */
export function answerCard(id: string, payload: { text: string } | { allow: boolean; note?: string }) {
  return invoke("agent_answer", { id, payload });
}

export function onAgentEvent(handler: (event: AgentEvent) => void): Promise<UnlistenFn> {
  return listen<AgentEvent>("agent://event", (e) => handler(e.payload));
}

/**
 * Thin client over the Rust agent. Conversation history, the system prompt
 * and the tool loop all live in the backend; this only sends text in and
 * gets the final reply back (tool progress arrives as events).
 */
export class AgentClient {
  constructor(
    private getApiKey: () => string,
    private getModel: () => string,
    private getPlannerModel: () => string,
    private getVerifierModel: () => string,
  ) {}

  reset() {
    void invoke("agent_reset");
  }

  cancel() {
    void invoke("agent_cancel");
  }

  async ask(text: string): Promise<string> {
    const apiKey = this.getApiKey();
    if (!apiKey) throw new Error("Add your OpenRouter API key in settings first.");
    const model = this.getModel().trim();
    if (!model) throw new Error("Set an OpenRouter model name in settings first.");
    // The planner and verifier models are optional; blank means "use the main one".
    return invoke<string>("agent_run", {
      apiKey,
      model,
      plannerModel: this.getPlannerModel().trim() || null,
      verifierModel: this.getVerifierModel().trim() || null,
      text,
    });
  }
}
