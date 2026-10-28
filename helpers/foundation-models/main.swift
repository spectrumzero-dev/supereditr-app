// supereditr-fm: SuperEditr's bridge to Apple's on-device model (the FoundationModels framework).
//
// The framework is Swift-only, so the Rust engine runs this small helper once and keeps it alive,
// exchanging one JSON object per line:
//
//   stdout, at start:  {"type":"hello","available":true}  or  {"available":false,"reason":"..."}
//   stdin:   {"type":"request","id":1,"system":"...","user":"...","maxTokens":512}
//            {"type":"cancel","id":1}   {"type":"prewarm"}   {"type":"status","id":2}
//   stdout:  {"type":"delta","id":1,"text":"..."}   {"type":"done","id":1,"text":"<full text>"}
//            {"type":"error","id":1,"code":"...","message":"..."}   {"type":"status","id":2,...}
//
// The helper exits when stdin closes, so it never outlives SuperEditr. It is built for macOS 13
// with FoundationModels weak-linked; on older systems it reports "osTooOld" instead of crashing.

import Foundation
#if canImport(FoundationModels)
import FoundationModels
#endif

final class Output: @unchecked Sendable {
    private let lock = NSLock()

    func send(_ object: [String: Any]) {
        guard let data = try? JSONSerialization.data(withJSONObject: object) else { return }
        lock.lock()
        defer { lock.unlock() }
        FileHandle.standardOutput.write(data)
        FileHandle.standardOutput.write(Data([0x0A]))
    }
}

final class Tasks: @unchecked Sendable {
    private let lock = NSLock()
    private var running: [Int: Task<Void, Never>] = [:]

    func add(_ id: Int, _ task: Task<Void, Never>) {
        lock.lock()
        running[id] = task
        lock.unlock()
    }

    func finish(_ id: Int) {
        lock.lock()
        running[id] = nil
        lock.unlock()
    }

    func cancel(_ id: Int) {
        lock.lock()
        let task = running.removeValue(forKey: id)
        lock.unlock()
        task?.cancel()
    }
}

let output = Output()
let tasks = Tasks()

func availability() -> [String: Any] {
    #if canImport(FoundationModels)
    if #available(macOS 26.0, *) {
        switch SystemLanguageModel.default.availability {
        case .available:
            return ["available": true]
        case .unavailable(let reason):
            let code: String
            switch reason {
            case .deviceNotEligible: code = "deviceNotEligible"
            case .appleIntelligenceNotEnabled: code = "appleIntelligenceNotEnabled"
            case .modelNotReady: code = "modelNotReady"
            @unknown default: code = "unavailable"
            }
            return ["available": false, "reason": code]
        @unknown default:
            return ["available": false, "reason": "unavailable"]
        }
    }
    return ["available": false, "reason": "osTooOld"]
    #else
    return ["available": false, "reason": "osTooOld"]
    #endif
}

#if canImport(FoundationModels)
@available(macOS 26.0, *)
func errorCode(_ error: Error) -> String {
    if let error = error as? LanguageModelSession.GenerationError {
        switch error {
        case .exceededContextWindowSize: return "contextWindow"
        case .guardrailViolation, .refusal: return "declined"
        case .unsupportedLanguageOrLocale: return "language"
        case .rateLimited: return "rateLimited"
        case .assetsUnavailable: return "modelNotReady"
        default: return "generation"
        }
    }
    return "generation"
}

@available(macOS 26.0, *)
func respond(id: Int, system: String, user: String, maxTokens: Int) async {
    defer { tasks.finish(id) }
    // Permissive guardrails are Apple's setting for transforming text the user wrote, which avoids
    // refusals on ordinary drafts that merely mention sensitive topics.
    let model = SystemLanguageModel(useCase: .general, guardrails: .permissiveContentTransformations)
    let session = LanguageModelSession(model: model, instructions: system)
    let options = GenerationOptions(samplingMode: .greedy, maximumResponseTokens: maxTokens)
    var sent = ""
    do {
        for try await snapshot in session.streamResponse(to: user, options: options) {
            if Task.isCancelled { return }
            let content = snapshot.content
            if content.hasPrefix(sent) {
                let delta = String(content.dropFirst(sent.count))
                if !delta.isEmpty {
                    output.send(["type": "delta", "id": id, "text": delta])
                }
            }
            sent = content
        }
        if !Task.isCancelled {
            output.send(["type": "done", "id": id, "text": sent])
        }
    } catch {
        if Task.isCancelled || error is CancellationError { return }
        output.send(["type": "error", "id": id, "code": errorCode(error), "message": String(describing: error)])
    }
}

@available(macOS 26.0, *)
func prewarm() {
    let model = SystemLanguageModel(useCase: .general, guardrails: .permissiveContentTransformations)
    LanguageModelSession(model: model).prewarm()
}
#endif

func handle(_ line: String) {
    guard let data = line.data(using: .utf8),
          let message = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
          let type = message["type"] as? String
    else { return }
    let id = message["id"] as? Int ?? 0
    switch type {
    case "status":
        var reply = availability()
        reply["type"] = "status"
        reply["id"] = id
        output.send(reply)
    case "cancel":
        tasks.cancel(id)
    case "prewarm":
        #if canImport(FoundationModels)
        if #available(macOS 26.0, *), SystemLanguageModel.default.isAvailable {
            prewarm()
        }
        #endif
    case "request":
        let system = message["system"] as? String ?? ""
        let user = message["user"] as? String ?? ""
        let maxTokens = message["maxTokens"] as? Int ?? 1024
        #if canImport(FoundationModels)
        if #available(macOS 26.0, *) {
            let status = availability()
            guard status["available"] as? Bool == true else {
                output.send(["type": "error", "id": id, "code": status["reason"] as? String ?? "unavailable", "message": "unavailable"])
                return
            }
            tasks.add(id, Task { await respond(id: id, system: system, user: user, maxTokens: maxTokens) })
            return
        }
        #endif
        output.send(["type": "error", "id": id, "code": "osTooOld", "message": "requires macOS 26"])
    default:
        break
    }
}

var hello = availability()
hello["type"] = "hello"
output.send(hello)

DispatchQueue.global(qos: .userInitiated).async {
    while let line = readLine(strippingNewline: true) {
        handle(line)
    }
    exit(0)
}
dispatchMain()
