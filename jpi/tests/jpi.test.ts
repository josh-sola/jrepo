import assert from "node:assert/strict";
import test from "node:test";

import jpi from "../extensions/jpi.ts";

test("the jpi command sends an info notification", async () => {
  let registeredName;
  let registeredCommand;

  const pi = {
    registerCommand(name, command) {
      registeredName = name;
      registeredCommand = command;
    },
  };

  jpi(pi);

  assert.equal(registeredName, "jpi");
  assert.ok(registeredCommand);

  const notifications = [];
  await registeredCommand.handler("", {
    ui: {
      notify(message, level) {
        notifications.push({ message, level });
      },
    },
  });

  assert.deepEqual(notifications, [{ message: "jpi is loaded.", level: "info" }]);
});
