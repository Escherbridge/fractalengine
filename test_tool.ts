import type { ExtensionAPI } from "@mariozechner/pi-coding-agent";

export default function(pi: ExtensionAPI) {
    pi.registerTool({
        name: "search",
        description: "Search for text in files",
        parameters: {
            type: "object",
            properties: {
                query: { type: "string" }
            },
            required: ["query"]
        },
        execute: async (args) => {
            return "ok";
        }
    });
}
