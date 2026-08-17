import Foundation
import Darwin
import CryptoKit

private struct BridgeRecord {
    var sessionID = ""
    var threadID: String?
    var ownerPID: Int32
    var label: String
    var cwd: String
    var color: String?
    var tab: Int?
    var createdAt: Double
    var updatedAt: Double
    var state = "waiting"
    var agents = 0
    var turnID: String?
    var turnActive = false
    var since: Double
}

private final class StateWriter {
    private let lock = NSLock()
    private let directory: URL
    private var record: BridgeRecord
    private var file: URL?
    private var collabIDs = Set<String>()
    private var pendingRequestIDs = Set<String>()

    init(directory: URL, ownerPID: Int32, environment: [String: String]) {
        self.directory = directory
        let now = Date().timeIntervalSince1970
        self.record = BridgeRecord(
            ownerPID: ownerPID,
            label: environment["PLANTER_LABEL"]
                ?? URL(fileURLWithPath: FileManager.default.currentDirectoryPath).lastPathComponent,
            cwd: FileManager.default.currentDirectoryPath,
            color: environment["PLANTER_COLOR"],
            tab: Int(environment["PLANTER_TAB_INDEX"] ?? ""),
            createdAt: now,
            updatedAt: now,
            since: now
        )
    }

    func bindThread(_ sessionID: String, threadID: String?) {
        lock.lock()
        defer { lock.unlock() }
        guard !sessionID.isEmpty else { return }
        if record.sessionID != sessionID, let file {
            try? FileManager.default.removeItem(at: file)
        }
        let now = Date().timeIntervalSince1970
        record.sessionID = sessionID
        record.threadID = threadID
        pendingRequestIDs.removeAll()
        collabIDs.removeAll()
        record.agents = 0
        record.turnID = nil
        record.turnActive = false
        record.state = "waiting"
        record.updatedAt = now
        record.since = now
        file = directory.appendingPathComponent("codex-\(safeName(sessionID)).json")
        writeLocked()
    }

    func observe(method: String, payload: Any, requestID: String? = nil) {
        lock.lock()
        defer { lock.unlock() }
        guard file != nil else { return }
        guard belongsToBoundThread(payload) else { return }
        let lower = method.lowercased()
        let now = Date().timeIntervalSince1970
        record.updatedAt = now
        if let turnID = string(for: ["turnId", "turn_id"], in: payload) { record.turnID = turnID }

        if lower == "thread/status/changed",
           let status = dictionary(payload)?["status"] as? [String: Any] {
            let type = (status["type"] as? String)?.lowercased() ?? ""
            if type.contains("active") || type.contains("working") {
                record.turnActive = true
            } else {
                record.turnActive = false
            }
            deriveStateLocked(now: now)
        } else if lower == "serverrequest/resolved" {
            if let resolvedID = identifier(for: ["requestId", "request_id"], in: payload) {
                pendingRequestIDs.remove(resolvedID)
            }
            deriveStateLocked(now: now)
        } else if isUserFacingRequest(lower, payload: payload), let requestID, !requestID.isEmpty {
            pendingRequestIDs.insert(requestID)
            deriveStateLocked(now: now)
        } else if let item = dictionary(payload)?["item"] as? [String: Any],
                  (item["type"] as? String) == "collabToolCall" {
            let id = item["id"] as? String ?? ""
            if lower == "item/started", !id.isEmpty {
                collabIDs.insert(id)
            }
            if lower == "item/completed" {
                collabIDs.remove(id)
            }
            record.agents = min(collabIDs.count, 2)
            deriveStateLocked(now: now)
        } else if lower == "turn/started" {
            record.turnActive = true
            deriveStateLocked(now: now)
        } else if lower == "turn/completed" || lower == "turn/interrupted" || lower == "turn/failed" {
            record.turnActive = false
            pendingRequestIDs.removeAll()
            deriveStateLocked(now: now)
        }
        writeLocked()
    }

    func remove() {
        lock.lock()
        defer { lock.unlock() }
        if let file { try? FileManager.default.removeItem(at: file) }
    }

    private func writeLocked() {
        guard let file, !record.sessionID.isEmpty else { return }
        try? FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        var json: [String: Any] = [
            "provider": "codex",
            "identity": "codex:\(record.sessionID)",
            "session_id": record.sessionID,
            "thread_id": record.threadID ?? NSNull(),
            "owner_pid": Int(record.ownerPID),
            "pid": Int(record.ownerPID),
            "label": record.label,
            "cwd": record.cwd,
            "state": record.state,
            "agents": record.agents,
            "turn": record.turnActive ? 1 : 0,
            "turn_active": record.turnActive,
            "created_at": record.createdAt,
            "updated_at": record.updatedAt,
            "since": record.since,
        ]
        json["color"] = record.color ?? NSNull()
        json["tab"] = record.tab ?? NSNull()
        json["turn_id"] = record.turnID ?? NSNull()
        guard let data = try? JSONSerialization.data(withJSONObject: json) else { return }
        try? data.write(to: file, options: .atomic)
    }

    private func belongsToBoundThread(_ payload: Any) -> Bool {
        guard let boundThreadID = record.threadID, let payloadThreadID = threadID(in: payload) else { return true }
        return boundThreadID == payloadThreadID
    }

    private func isUserFacingRequest(_ method: String, payload: Any) -> Bool {
        switch method {
        case "item/commandexecution/requestapproval",
             "item/filechange/requestapproval",
             "item/permissions/requestapproval",
             "mcpserver/elicitation/request",
             "applypatchapproval",
             "execcommandapproval":
            return true
        case "item/tool/requestuserinput":
            return boolValue(for: ["isBlocking", "is_blocking"], in: payload) ?? true
        default:
            return false
        }
    }

    private func deriveStateLocked(now: Double) {
        let state: String
        if !pendingRequestIDs.isEmpty {
            state = "attention"
        } else if record.turnActive || record.agents > 0 {
            state = "working"
        } else {
            state = "waiting"
        }
        if record.state != state {
            record.state = state
            record.since = state == "working" ? 0 : now
        } else if state == "working" {
            record.since = 0
        }
    }
}

private func safeName(_ value: String) -> String {
    value.unicodeScalars.map {
        CharacterSet.alphanumerics.contains($0) || "._-".unicodeScalars.contains($0)
            ? String($0) : "_"
    }.joined()
}

private func dictionary(_ value: Any) -> [String: Any]? { value as? [String: Any] }

private func string(for keys: Set<String>, in value: Any) -> String? {
    if let dict = dictionary(value) {
        for (key, child) in dict {
            if keys.contains(key), let value = child as? String { return value }
            if let found = string(for: keys, in: child) { return found }
        }
    } else if let values = value as? [Any] {
        for child in values { if let found = string(for: keys, in: child) { return found } }
    }
    return nil
}

private func identifier(for keys: Set<String>, in value: Any) -> String? {
    if let dict = dictionary(value) {
        for (key, child) in dict {
            if keys.contains(key), let value = jsonRPCID(child) { return value }
            if let found = identifier(for: keys, in: child) { return found }
        }
    } else if let values = value as? [Any] {
        for child in values { if let found = identifier(for: keys, in: child) { return found } }
    }
    return nil
}

private func boolValue(for keys: Set<String>, in value: Any) -> Bool? {
    if let dict = dictionary(value) {
        for (key, child) in dict {
            if keys.contains(key), let value = child as? Bool { return value }
            if let found = boolValue(for: keys, in: child) { return found }
        }
    } else if let values = value as? [Any] {
        for child in values { if let found = boolValue(for: keys, in: child) { return found } }
    }
    return nil
}

private func threadID(in value: Any) -> String? {
    if let dict = dictionary(value) {
        if let id = dict["threadId"] as? String ?? dict["thread_id"] as? String { return id }
        if let thread = dict["thread"] as? [String: Any], let id = thread["id"] as? String { return id }
        for child in dict.values { if let found = threadID(in: child) { return found } }
    } else if let values = value as? [Any] {
        for child in values { if let found = threadID(in: child) { return found } }
    }
    return nil
}

private func threadIdentity(in value: Any) -> (sessionID: String, threadID: String?)? {
    if let dict = dictionary(value) {
        if let thread = dict["thread"] as? [String: Any], let id = thread["id"] as? String {
            return (thread["sessionId"] as? String ?? id, id)
        }
        if let id = dict["threadId"] as? String { return (id, id) }
        for child in dict.values { if let found = threadIdentity(in: child) { return found } }
    } else if let values = value as? [Any] {
        for child in values { if let found = threadIdentity(in: child) { return found } }
    }
    return nil
}

private func jsonRPCID(_ value: Any?) -> String? {
    guard let value else { return nil }
    if let value = value as? String { return value }
    if let value = value as? NSNumber, CFGetTypeID(value) != CFBooleanGetTypeID() {
        return value.stringValue
    }
    return nil
}

private func webSocketKey(in request: String) -> String? {
    guard request.hasPrefix("GET ") else { return nil }
    let header = "Sec-WebSocket-Key:"
    guard let line = request.components(separatedBy: "\r\n").first(where: {
        $0.range(of: header, options: [.anchored, .caseInsensitive]) != nil
    }) else { return nil }
    let key = String(line.dropFirst(header.count))
        .trimmingCharacters(in: .whitespacesAndNewlines)
    return key.isEmpty ? nil : key
}

private final class WebSocketBridge {
    private let listener: Int32
    private let socketPath: String
    private let app: Process
    private let input: FileHandle
    private let writer: StateWriter
    private let ownerPID: Int32
    private let lock = NSLock()
    private var client: Int32 = -1
    private var requestMethods: [String: String] = [:]
    private var receiveBuffer = Data()
    private var appBuffer = Data()
    private var fragmentedOpcode: UInt8?
    private var fragmentedPayload = Data()
    private var terminationSource: DispatchSourceSignal?
    private var stopped = false

    init(listener: Int32, socketPath: String, app: Process, input: FileHandle, writer: StateWriter, ownerPID: Int32) {
        self.listener = listener
        self.socketPath = socketPath
        self.app = app
        self.input = input
        self.writer = writer
        self.ownerPID = ownerPID
    }

    func run() {
        _ = signal(SIGTERM, SIG_IGN)
        let source = DispatchSource.makeSignalSource(signal: SIGTERM, queue: .global())
        source.setEventHandler { [weak self] in self?.stop() }
        source.resume()
        terminationSource = source
        app.standardOutput.map {
            ($0 as? Pipe)?.fileHandleForReading.readabilityHandler = { [weak self] handle in
                self?.appOutput(handle.availableData)
            }
        }
        app.terminationHandler = { [weak self] _ in self?.stop() }
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in self?.acceptLoop() }
        DispatchQueue.global(qos: .utility).async { [weak self] in self?.watchOwner() }
        dispatchMain()
    }

    private func acceptLoop() {
        while !isStopped() {
            let fd = accept(listener, nil, nil)
            guard fd >= 0 else { continue }
            lock.lock(); let occupied = client >= 0; lock.unlock()
            if occupied || !handshake(fd) { Darwin.close(fd); continue }
            lock.lock(); client = fd; let frames = takeFrames(&receiveBuffer); lock.unlock()
            for frame in frames { handleFrame(frame) }
            readClient(fd)
            lock.lock(); if client == fd { client = -1 }; lock.unlock()
            Darwin.close(fd)
            stop()
        }
    }

    private func handshake(_ fd: Int32) -> Bool {
        var request = Data()
        var chunk = [UInt8](repeating: 0, count: 2048)
        while request.count < 16_384 {
            let count = recv(fd, &chunk, chunk.count, 0)
            guard count > 0 else { return handshakeFailure("connection closed before upgrade") }
            request.append(chunk, count: count)
            if request.range(of: Data("\r\n\r\n".utf8)) != nil { break }
        }
        guard let range = request.range(of: Data("\r\n\r\n".utf8)) else {
            return handshakeFailure("upgrade headers were too large")
        }
        guard let text = String(data: request.subdata(in: 0..<range.upperBound), encoding: .utf8)
        else { return handshakeFailure("invalid HTTP upgrade request") }
        guard let key = webSocketKey(in: text) else {
            return handshakeFailure("invalid WebSocket upgrade request")
        }
        lock.lock(); receiveBuffer.append(request.subdata(in: range.upperBound..<request.count)); lock.unlock()
        let challenge = key + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"
        let accept = Data(Insecure.SHA1.hash(data: Data(challenge.utf8))).base64EncodedString()
        let response = "HTTP/1.1 101 Switching Protocols\r\n"
            + "Upgrade: websocket\r\nConnection: Upgrade\r\n"
            + "Sec-WebSocket-Accept: \(accept)\r\n\r\n"
        return sendRaw(fd, Data(response.utf8))
    }

    private func handshakeFailure(_ reason: String) -> Bool {
        FileHandle.standardError.write(Data("planter-codex-bridge: \(reason)\n".utf8))
        return false
    }

    private func readClient(_ fd: Int32) {
        var bytes = [UInt8](repeating: 0, count: 8192)
        while !isStopped() {
            let count = recv(fd, &bytes, bytes.count, 0)
            guard count > 0 else { return }
            lock.lock()
            receiveBuffer.append(bytes, count: count)
            let frames = takeFrames(&receiveBuffer)
            lock.unlock()
            for frame in frames { handleFrame(frame) }
        }
    }

    private func handleFrame(_ frame: (opcode: UInt8, final: Bool, payload: Data)) {
        if frame.opcode == 8 { sendFrame(frame.payload, opcode: 8); return }
        if frame.opcode == 9 { sendFrame(frame.payload, opcode: 10); return }
        if frame.opcode == 10 { return }
        if frame.opcode == 0, fragmentedOpcode != nil {
            fragmentedPayload.append(frame.payload)
            if frame.final {
                handleClientMessage(fragmentedPayload)
                fragmentedOpcode = nil
                fragmentedPayload.removeAll()
            }
        } else if frame.opcode == 1 || frame.opcode == 2 {
            if frame.final { handleClientMessage(frame.payload) }
            else { fragmentedOpcode = frame.opcode; fragmentedPayload = frame.payload }
        }
    }

    private func handleClientMessage(_ data: Data) {
        do {
            try input.write(contentsOf: data)
            try input.write(contentsOf: Data([10]))
        } catch {
            stop()
            return
        }
        if let json = try? JSONSerialization.jsonObject(with: data), let dict = json as? [String: Any],
           let method = dict["method"] as? String {
            if let id = dict["id"] { lock.lock(); requestMethods["\(id)"] = method; lock.unlock() }
            writer.observe(method: method, payload: dict["params"] as Any)
        }
    }

    private func appOutput(_ data: Data) {
        guard !data.isEmpty else { return }
        lock.lock(); appBuffer.append(data)
        let newline = Data([10])
        var messages: [Data] = []
        while let range = appBuffer.range(of: newline) {
            messages.append(appBuffer.subdata(in: 0..<range.lowerBound))
            appBuffer.removeSubrange(0...range.lowerBound)
        }
        lock.unlock()
        for message in messages where !message.isEmpty {
            sendFrame(message, opcode: 1)
            observeAppMessage(message)
        }
    }

    private func observeAppMessage(_ data: Data) {
        guard let json = try? JSONSerialization.jsonObject(with: data),
              let dict = json as? [String: Any]
        else { return }
        if let method = dict["method"] as? String {
            writer.observe(
                method: method,
                payload: dict["params"] as Any,
                requestID: jsonRPCID(dict["id"])
            )
        }
        if let id = dict["id"] {
            let key = "\(id)"
            lock.lock(); let request = requestMethods.removeValue(forKey: key); lock.unlock()
            if let method = request,
               method == "thread/start" || method == "thread/resume" || method == "thread/fork",
               let identity = threadIdentity(in: dict["result"] as Any) {
                writer.bindThread(identity.sessionID, threadID: identity.threadID)
            }
        }
    }

    private func takeFrames(_ buffer: inout Data) -> [(opcode: UInt8, final: Bool, payload: Data)] {
        var frames: [(UInt8, Bool, Data)] = []
        while buffer.count >= 2 {
            let header = [UInt8](buffer.prefix(2))
            let opcode = header[0] & 0x0f
            let final = header[0] & 0x80 != 0
            let masked = header[1] & 0x80 != 0
            var index = 2
            var length = Int(header[1] & 0x7f)
            if length == 126 {
                guard buffer.count >= 4 else { break }
                length = Int(buffer[2]) << 8 | Int(buffer[3])
                index = 4
            }
            if length == 127 {
                guard buffer.count >= 10 else { break }
                var wide: UInt64 = 0
                for byte in buffer[2..<10] { wide = (wide << 8) | UInt64(byte) }
                guard wide <= UInt64(Int.max) else { buffer.removeAll(); break }
                length = Int(wide); index = 10
            }
            let maskLength = masked ? 4 : 0
            guard buffer.count >= index + maskLength + length else { break }
            let mask = masked ? Array(buffer[index..<(index + 4)]) : []
            index += maskLength
            var payload = Data(buffer[index..<(index + length)])
            if masked { for i in payload.indices { payload[i] ^= mask[i % 4] } }
            buffer.removeSubrange(0..<(index + length)); frames.append((opcode, final, payload))
        }
        return frames
    }

    private func sendFrame(_ data: Data, opcode: UInt8) {
        lock.lock(); let fd = client
        guard fd >= 0 else { lock.unlock(); return }
        var frame = Data([0x80 | opcode])
        if data.count < 126 { frame.append(UInt8(data.count)) }
        else if data.count <= 65_535 {
            frame.append(126)
            frame.append(UInt8((data.count >> 8) & 0xff))
            frame.append(UInt8(data.count & 0xff))
        }
        else {
            frame.append(127)
            let length = UInt64(data.count)
            for shift in stride(from: 56, through: 0, by: -8) { frame.append(UInt8((length >> UInt64(shift)) & 0xff)) }
        }
        frame.append(data); _ = sendRaw(fd, frame); lock.unlock()
    }

    private func sendRaw(_ fd: Int32, _ data: Data) -> Bool {
        data.withUnsafeBytes { pointer in
            guard let base = pointer.baseAddress else { return false }
            var offset = 0
            while offset < data.count {
                let written = send(fd, base.advanced(by: offset), data.count - offset, 0)
                guard written > 0 else { return false }
                offset += written
            }
            return true
        }
    }

    private func watchOwner() {
        while !isStopped() {
            if kill(ownerPID, 0) != 0 && errno != EPERM {
                stop()
                return
            }
            sleep(1)
        }
    }

    private func isStopped() -> Bool {
        lock.lock()
        defer { lock.unlock() }
        return stopped
    }

    private func shutdownApp() {
        guard app.isRunning else { return }
        app.terminate()
        let deadline = Date().addingTimeInterval(1)
        while app.isRunning && Date() < deadline {
            Thread.sleep(forTimeInterval: 0.05)
        }
        if app.isRunning {
            let childPID = app.processIdentifier
            if childPID > 1, kill(childPID, 0) == 0 || errno == EPERM {
                _ = kill(childPID, SIGKILL)
            }
        }
        app.waitUntilExit()
    }

    private func stop() {
        lock.lock()
        if stopped {
            lock.unlock()
            return
        }
        stopped = true
        let fd = client
        lock.unlock()

        if fd >= 0 { Darwin.close(fd) }
        Darwin.close(listener)
        unlink(socketPath)
        writer.remove()
        shutdownApp()
        exit(0)
    }
}

private func ownerPID(arguments: [String], environment: [String: String]) -> Int32 {
    if let index = arguments.firstIndex(of: "--owner-pid"),
       index + 1 < arguments.count,
       let pid = Int32(arguments[index + 1]) {
        return pid
    }
    if let pid = Int32(environment["PLANTER_OWNER_PID"] ?? "") { return pid }
    return getppid()
}

private func makeListener(path: String) -> Int32? {
    let fd = socket(AF_UNIX, SOCK_STREAM, 0); guard fd >= 0 else { return nil }; unlink(path)
    var address = sockaddr_un(); address.sun_family = sa_family_t(AF_UNIX)
    let bytes = Array(path.utf8) + [0]
    guard bytes.count <= MemoryLayout.size(ofValue: address.sun_path) else { Darwin.close(fd); return nil }
    withUnsafeMutableBytes(of: &address.sun_path) { $0.copyBytes(from: bytes) }
    let length = socklen_t(MemoryLayout<sockaddr_un>.offset(of: \.sun_path)! + bytes.count)
    address.sun_len = UInt8(length)
    let bound = withUnsafePointer(to: &address) {
        bind(fd, UnsafeRawPointer($0).assumingMemoryBound(to: sockaddr.self), length)
    } == 0
    guard bound else {
        Darwin.close(fd)
        return nil
    }
    _ = chmod(path, S_IRUSR | S_IWUSR)
    guard listen(fd, 1) == 0 else {
        Darwin.close(fd)
        unlink(path)
        return nil
    }
    return fd
}

private func runSelfTest() -> Never {
    let directory = URL(fileURLWithPath: NSTemporaryDirectory())
        .appendingPathComponent("planter-codex-selftest-\(getpid())")
    func finish(_ message: String, status: Int32) -> Never {
        try? FileManager.default.removeItem(at: directory)
        let handle = status == 0 ? FileHandle.standardOutput : FileHandle.standardError
        handle.write(Data((message + "\n").utf8))
        exit(status)
    }
    func record(_ file: URL) -> [String: Any]? {
        guard let data = try? Data(contentsOf: file) else { return nil }
        return (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
    }

    let upgrade = "GET / HTTP/1.1\r\nHost: localhost\r\n"
        + "Upgrade: websocket\r\nConnection: Upgrade\r\n"
        + "Sec-WebSocket-Key: MDEyMzQ1Njc4OWFiY2RlZg==\r\n\r\n"
    guard webSocketKey(in: upgrade) == "MDEyMzQ1Njc4OWFiY2RlZg==" else {
        finish("planter-codex-bridge self-test failed: WebSocket upgrade", status: 1)
    }

    let writer = StateWriter(directory: directory, ownerPID: getpid(), environment: [:])
    writer.bindThread("thr_test", threadID: "thr_test")
    let file = directory.appendingPathComponent("codex-thr_test.json")
    let thread: [String: Any] = ["threadId": "thr_test"]
    writer.observe(method: "thread/status/changed", payload: [
        "threadId": "thr_test",
        "status": ["type": "active", "activeFlags": ["waitingOnApproval"]],
    ])
    guard record(file)?["state"] as? String == "working" else {
        finish("planter-codex-bridge self-test failed: active flag", status: 1)
    }
    writer.observe(method: "item/autoApprovalReview/started", payload: thread)
    writer.observe(method: "item/autoApprovalReview/completed", payload: thread)
    guard record(file)?["state"] as? String == "working" else {
        finish("planter-codex-bridge self-test failed: auto approval review", status: 1)
    }
    writer.observe(
        method: "item/commandExecution/requestApproval",
        payload: thread,
        requestID: "request-1"
    )
    guard record(file)?["state"] as? String == "attention" else {
        finish("planter-codex-bridge self-test failed: request approval", status: 1)
    }
    writer.observe(method: "item/autoApprovalReview/started", payload: thread)
    guard record(file)?["state"] as? String == "attention" else {
        finish("planter-codex-bridge self-test failed: request persistence", status: 1)
    }
    writer.observe(method: "serverRequest/resolved", payload: [
        "threadId": "thr_test", "requestId": "other-request",
    ])
    guard record(file)?["state"] as? String == "attention" else {
        finish("planter-codex-bridge self-test failed: unrelated resolution", status: 1)
    }
    writer.observe(method: "serverRequest/resolved", payload: [
        "threadId": "thr_test", "requestId": "request-1",
    ])
    guard record(file)?["state"] as? String == "working" else {
        finish("planter-codex-bridge self-test failed: matching resolution", status: 1)
    }
    writer.observe(
        method: "item/permissions/requestApproval",
        payload: thread,
        requestID: jsonRPCID(4)
    )
    writer.observe(method: "serverRequest/resolved", payload: [
        "threadId": "thr_test", "requestId": 4,
    ])
    guard record(file)?["state"] as? String == "working" else {
        finish("planter-codex-bridge self-test failed: numeric resolution", status: 1)
    }
    writer.observe(
        method: "item/fileChange/requestApproval",
        payload: thread,
        requestID: "request-2"
    )
    writer.observe(
        method: "mcpServer/elicitation/request",
        payload: thread,
        requestID: "request-3"
    )
    writer.observe(method: "serverRequest/resolved", payload: [
        "threadId": "thr_test", "requestId": "request-2",
    ])
    guard record(file)?["state"] as? String == "attention" else {
        finish("planter-codex-bridge self-test failed: concurrent resolution", status: 1)
    }
    writer.observe(method: "serverRequest/resolved", payload: [
        "threadId": "thr_test", "requestId": "request-3",
    ])
    guard record(file)?["state"] as? String == "working" else {
        finish("planter-codex-bridge self-test failed: concurrent requests", status: 1)
    }
    writer.observe(
        method: "item/tool/requestUserInput",
        payload: ["threadId": "thr_test", "isBlocking": false],
        requestID: "nonblocking-request"
    )
    guard record(file)?["state"] as? String == "working" else {
        finish("planter-codex-bridge self-test failed: nonblocking input", status: 1)
    }
    writer.observe(
        method: "item/tool/requestUserInput",
        payload: ["threadId": "thr_test", "isBlocking": true],
        requestID: "blocking-input-request"
    )
    guard record(file)?["state"] as? String == "attention" else {
        finish("planter-codex-bridge self-test failed: blocking input", status: 1)
    }
    writer.observe(method: "thread/status/changed", payload: [
        "threadId": "thr_test",
        "status": ["type": "active", "activeFlags": []],
    ])
    guard record(file)?["state"] as? String == "attention" else {
        finish("planter-codex-bridge self-test failed: status request persistence", status: 1)
    }
    writer.observe(method: "serverRequest/resolved", payload: [
        "threadId": "thr_test", "requestId": "blocking-input-request",
    ])
    guard record(file)?["state"] as? String == "working" else {
        finish("planter-codex-bridge self-test failed: blocking input resolution", status: 1)
    }
    writer.observe(
        method: "item/permissions/requestApproval",
        payload: ["threadId": "thr_other"],
        requestID: "other-thread-request"
    )
    guard record(file)?["state"] as? String == "working" else {
        finish("planter-codex-bridge self-test failed: cross-thread request", status: 1)
    }
    writer.observe(method: "turn/failed", payload: thread)
    guard record(file)?["state"] as? String == "waiting" else {
        finish("planter-codex-bridge self-test failed: failed turn", status: 1)
    }
    let collab: [String: Any] = ["item": ["type": "collabToolCall", "id": "agent-1"]]
    writer.observe(method: "item/started", payload: collab)
    writer.observe(method: "item/started", payload: collab)
    guard record(file)?["agents"] as? Int == 1 else {
        finish("planter-codex-bridge self-test failed: collab start", status: 1)
    }
    writer.observe(method: "item/completed", payload: collab)
    guard record(file)?["agents"] as? Int == 0 else {
        finish("planter-codex-bridge self-test failed: collab completion", status: 1)
    }
    writer.observe(
        method: "item/fileChange/requestApproval",
        payload: thread,
        requestID: "request-before-rebind"
    )
    writer.bindThread("thr_rebound", threadID: "thr_rebound")
    let rebound = directory.appendingPathComponent("codex-thr_rebound.json")
    guard !FileManager.default.fileExists(atPath: file.path),
          FileManager.default.fileExists(atPath: rebound.path),
          record(rebound)?["provider"] as? String == "codex",
          record(rebound)?["state"] as? String == "waiting",
          record(rebound)?["agents"] as? Int == 0
    else {
        finish("planter-codex-bridge self-test failed: rebind", status: 1)
    }
    writer.remove()
    guard !FileManager.default.fileExists(atPath: rebound.path) else {
        finish("planter-codex-bridge cleanup self-test failed", status: 1)
    }
    finish("planter-codex-bridge self-test passed", status: 0)
}

if CommandLine.arguments.dropFirst().contains("--self-test") { runSelfTest() }

let environment = ProcessInfo.processInfo.environment
_ = signal(SIGPIPE, SIG_IGN)
let defaultDirectory = FileManager.default.homeDirectoryForCurrentUser
    .appendingPathComponent(".claude/planter").path
let directory = URL(fileURLWithPath: environment["PLANTER_STATE_DIR"]
    ?? environment["CLAUDE_PLANTER_DIR"] ?? defaultDirectory)
try? FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
let owner = ownerPID(arguments: Array(CommandLine.arguments.dropFirst()), environment: environment)
let socketPath = directory.appendingPathComponent("codex-bridge-\(getpid()).sock").path
guard let listener = makeListener(path: socketPath) else {
    FileHandle.standardError.write(Data("planter-codex-bridge: could not create socket\n".utf8))
    exit(1)
}
let app = Process()
app.executableURL = URL(fileURLWithPath: "/usr/bin/env")
app.arguments = ["codex", "app-server", "--listen", "stdio://"]
let inputPipe = Pipe()
let outputPipe = Pipe()
app.standardInput = inputPipe
app.standardOutput = outputPipe
guard (try? app.run()) != nil else {
    Darwin.close(listener)
    unlink(socketPath)
    FileHandle.standardError.write(Data("planter-codex-bridge: could not start codex app-server\n".utf8))
    exit(1)
}
private let writer = StateWriter(directory: directory, ownerPID: owner, environment: environment)
print("unix://\(socketPath)")
fflush(stdout)
WebSocketBridge(
    listener: listener,
    socketPath: socketPath,
    app: app,
    input: inputPipe.fileHandleForWriting,
    writer: writer,
    ownerPID: owner
).run()
