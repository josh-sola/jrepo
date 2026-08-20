import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

export default function jpi(pi: ExtensionAPI) {
  pi.registerCommand("jpi", {
    description: "Confirm that jpi is loaded",
    handler: async (_args, ctx) => {
      ctx.ui.notify("jpi is loaded.", "info");
    },
  });
}
