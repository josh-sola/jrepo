import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { truncateToWidth, visibleWidth } from "@earendil-works/pi-tui";

import { createStatusExtension } from "./extension.ts";

export default function jpiStatus(pi: ExtensionAPI) {
  const extension = createStatusExtension(
    (command, args, options) => pi.exec(command, args, options),
    { truncateToWidth, visibleWidth },
  );

  pi.on("session_start", extension.onSessionStart);
  pi.registerCommand("jpi-status", {
    description: "Show, refresh, or reload the custom footer",
    handler: extension.onCommand,
  });
}
