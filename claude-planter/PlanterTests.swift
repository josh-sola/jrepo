import AppKit

@main
struct PlanterTests {
    struct CommandResult {
        var status: Int32
        var stdout: String
        var stderr: String
    }

    static func fail(_ message: String) -> Never {
        FileHandle.standardError.write("PlanterTests: \(message)\n".data(using: .utf8)!)
        exit(1)
    }

    static func expect(_ condition: @autoclosure () -> Bool, _ message: String) {
        if !condition() { fail(message) }
    }

    static func withStateDirectory(_ body: (URL) -> Void) {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("planter-tests-\(UUID().uuidString)")
        do {
            try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        } catch {
            fail("could not create temporary state directory: \(error)")
        }
        defer { try? FileManager.default.removeItem(at: directory) }
        body(directory)
    }

    static func writeJSON(_ object: Any, to file: URL) {
        do {
            let data = try JSONSerialization.data(withJSONObject: object)
            try data.write(to: file)
        } catch {
            fail("could not write fixture: \(error)")
        }
    }

    static func writeCodexRecord(
        _ stateDirectory: URL, id: String, color: String? = nil, createdAt: Double
    ) {
        var record: [String: Any] = [
            "provider": "codex",
            "identity": "codex:\(id)",
            "session_id": id,
            "label": id,
            "state": "working",
            "created_at": createdAt,
            "updated_at": createdAt,
        ]
        if let color { record["color"] = color }
        writeJSON(record, to: stateDirectory.appendingPathComponent("codex-\(id).json"))
    }

    static func runPlanter(
        _ binary: String, arguments: [String], environment overrides: [String: String?]
    ) -> CommandResult {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: binary)
        process.arguments = arguments
        var environment = ProcessInfo.processInfo.environment
        for (key, value) in overrides {
            if let value {
                environment[key] = value
            } else {
                environment.removeValue(forKey: key)
            }
        }
        process.environment = environment

        let stdout = Pipe()
        let stderr = Pipe()
        process.standardOutput = stdout
        process.standardError = stderr
        let outputGroup = DispatchGroup()
        var stdoutData = Data()
        var stderrData = Data()
        outputGroup.enter()
        DispatchQueue.global(qos: .utility).async {
            stdoutData = stdout.fileHandleForReading.readDataToEndOfFile()
            outputGroup.leave()
        }
        outputGroup.enter()
        DispatchQueue.global(qos: .utility).async {
            stderrData = stderr.fileHandleForReading.readDataToEndOfFile()
            outputGroup.leave()
        }
        let exited = DispatchSemaphore(value: 0)
        process.terminationHandler = { _ in exited.signal() }
        do {
            try process.run()
        } catch {
            fail("could not run planter: \(error)")
        }
        guard exited.wait(timeout: .now() + 2) == .success else {
            process.terminate()
            _ = exited.wait(timeout: .now() + 1)
            fail("planter did not exit within 2 seconds")
        }
        outputGroup.wait()
        return CommandResult(
            status: process.terminationStatus,
            stdout: String(data: stdoutData, encoding: .utf8) ?? "",
            stderr: String(data: stderrData, encoding: .utf8) ?? ""
        )
    }

    static func testAutomaticPolicy() {
        var captured: [String] = []
        let color = ColorResolver.automaticColor(for: ["red", "red", "blue"]) { choices in
            captured = choices
            return choices[0]
        }
        expect(paletteIsValid(color), "automatic color was not in the palette")
        expect(!captured.contains("red"), "a color tied for most used was eligible")
        expect(captured.allSatisfy(paletteIsValid), "automatic candidates included an invalid color")

        let allNames = palette.map(\.name)
        let tiedColor = ColorResolver.automaticColor(for: allNames) { choices in
            captured = choices
            return choices[0]
        }
        expect(paletteIsValid(tiedColor), "tied automatic color was not in the palette")
        expect(captured == allNames, "all palette colors were not eligible when usage tied")

        for effectiveColors in [[], allNames, ["green", "green", "green"]] {
            let result = ColorResolver.automaticColor(for: effectiveColors) { $0[0] }
            expect(paletteIsValid(result), "automatic selection returned an invalid color")
        }
    }

    static func testCLI(_ binary: String) {
        withStateDirectory { stateDirectory in
            let valid = runPlanter(binary, arguments: ["--resolve-color"], environment: [
                "PLANTER_COLOR": "purple",
                "PLANTER_STATE_DIR": stateDirectory.path,
            ])
            expect(valid.status == 0, "valid override failed: \(valid.stderr)")
            expect(valid.stdout == "purple\n", "valid override did not return exactly purple")
            expect((try? FileManager.default.contentsOfDirectory(atPath: stateDirectory.path)) == [],
                   "valid override loaded or changed state")
        }

        for override in [nil, "", "violet"] {
            withStateDirectory { stateDirectory in
                let result = runPlanter(binary, arguments: ["--resolve-color"], environment: [
                    "PLANTER_COLOR": override,
                    "PLANTER_STATE_DIR": stateDirectory.path,
                ])
                expect(result.status == 0, "automatic fallback failed: \(result.stderr)")
                expect(result.stdout.hasSuffix("\n"), "automatic fallback omitted its newline")
                let token = String(result.stdout.dropLast())
                expect(paletteIsValid(token), "automatic fallback did not return a palette token")
                expect((try? FileManager.default.contentsOfDirectory(atPath: stateDirectory.path)) == [],
                       "automatic preflight created a returned-color reservation")
            }
        }

        withStateDirectory { stateDirectory in
            writeCodexRecord(stateDirectory, id: "a", color: "red", createdAt: 1)
            writeCodexRecord(stateDirectory, id: "b", createdAt: 2)
            writeCodexRecord(stateDirectory, id: "c", color: "blue", createdAt: 3)
            writeJSON(["codex:b": "red"], to: stateDirectory.appendingPathComponent("automatic-colors.json"))

            let result = runPlanter(binary, arguments: ["--resolve-color"], environment: [
                "PLANTER_COLOR": "invalid",
                "PLANTER_STATE_DIR": stateDirectory.path,
            ])
            expect(result.status == 0, "state-backed resolution failed: \(result.stderr)")
            let token = String(result.stdout.dropLast())
            expect(paletteIsValid(token), "state-backed resolution did not return a palette token")
            expect(token != "red", "state-backed resolution ignored explicit and persisted effective colors")
        }

        withStateDirectory { legacyDirectory in
            writeCodexRecord(legacyDirectory, id: "legacy", createdAt: 1)
            let result = runPlanter(binary, arguments: ["--resolve-color"], environment: [
                "PLANTER_COLOR": "invalid",
                "PLANTER_STATE_DIR": nil,
                "CLAUDE_PLANTER_DIR": legacyDirectory.path,
            ])
            expect(result.status == 0, "legacy state-directory resolution failed: \(result.stderr)")
            expect(paletteIsValid(String(result.stdout.dropLast())),
                   "legacy state-directory resolution did not return a palette token")
            expect(FileManager.default.fileExists(
                atPath: legacyDirectory.appendingPathComponent("automatic-colors.json").path
            ), "legacy state directory was not loaded")
        }

        withStateDirectory { stateDirectory in
            withStateDirectory { legacyDirectory in
                writeCodexRecord(stateDirectory, id: "current", createdAt: 1)
                writeCodexRecord(legacyDirectory, id: "legacy", createdAt: 1)
                let result = runPlanter(binary, arguments: ["--resolve-color"], environment: [
                    "PLANTER_COLOR": "invalid",
                    "PLANTER_STATE_DIR": stateDirectory.path,
                    "CLAUDE_PLANTER_DIR": legacyDirectory.path,
                ])
                expect(result.status == 0, "state-directory precedence resolution failed: \(result.stderr)")
                expect(FileManager.default.fileExists(
                    atPath: stateDirectory.appendingPathComponent("automatic-colors.json").path
                ), "PLANTER_STATE_DIR was not loaded")
                expect(!FileManager.default.fileExists(
                    atPath: legacyDirectory.appendingPathComponent("automatic-colors.json").path
                ), "CLAUDE_PLANTER_DIR won over PLANTER_STATE_DIR")
            }
        }

        withStateDirectory { stateDirectory in
            let mixed = runPlanter(binary, arguments: ["--resolve-color", "--help"], environment: [
                "PLANTER_STATE_DIR": stateDirectory.path,
            ])
            expect(mixed.status != 0, "mixed resolve-color options succeeded")
            expect(mixed.stdout.isEmpty, "mixed resolve-color options wrote stdout")
            expect(mixed.stderr.contains("--resolve-color"), "mixed resolve-color error was unclear")
        }
    }

    static func main() {
        let arguments = Array(CommandLine.arguments.dropFirst())
        guard arguments.count == 1 else { fail("expected the planter binary path") }
        testAutomaticPolicy()
        testCLI(arguments[0])
    }
}
