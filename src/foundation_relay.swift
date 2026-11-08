import Foundation

#if canImport(FoundationModels)
import FoundationModels
#endif

/// In-process bridge to the system language model. `anyr` embeds this dylib and
/// loads it only when the OS can run it. Relay frames stay on the Rust side.
///
/// @_cdecl exports: anyr_fm_status, anyr_fm_complete.

@_cdecl("anyr_fm_status")
public func anyr_fm_status() -> Int32 {
    guard #available(macOS 26.0, *) else { return 0 }
    #if canImport(FoundationModels)
    return AnyrFm.status()
    #else
    return 0
    #endif
}

@_cdecl("anyr_fm_complete")
public func anyr_fm_complete(
    instructions: UnsafePointer<CChar>?,
    prompt: UnsafePointer<CChar>?,
    stream: Int32,
    onText: @convention(c) (UnsafeMutableRawPointer?, UnsafePointer<CChar>?) -> Void,
    onError: @convention(c) (UnsafeMutableRawPointer?, UnsafePointer<CChar>?) -> Void,
    shouldStop: @convention(c) (UnsafeMutableRawPointer?) -> Int32,
    ctx: UnsafeMutableRawPointer?
) -> Int32 {
    let instructionsCopy = instructions.map { String(cString: $0) } ?? ""
    let promptCopy = prompt.map { String(cString: $0) } ?? ""
    let callbacks = Callbacks(onText: onText, onError: onError, shouldStop: shouldStop, ctx: ctx)
    let box = StatusBox()
    let sem = DispatchSemaphore(value: 0)
    let task = Task.detached {
        defer { sem.signal() }
        guard #available(macOS 26.0, *) else {
            callbacks.fail("system model is unavailable")
            box.rc = 1
            return
        }
        #if canImport(FoundationModels)
        box.rc = await AnyrFm.generate(
            instructions: instructionsCopy,
            prompt: promptCopy,
            stream: stream != 0,
            callbacks: callbacks
        )
        #else
        callbacks.fail("system model is unavailable")
        box.rc = 1
        #endif
    }
    while sem.wait(timeout: .now() + .milliseconds(100)) == .timedOut {
        if callbacks.stopped() {
            task.cancel()
            sem.wait()
            return 2
        }
    }
    return box.rc
}

private final class StatusBox: @unchecked Sendable {
    var rc: Int32 = 0
}

private final class Callbacks: @unchecked Sendable {
    let onText: @convention(c) (UnsafeMutableRawPointer?, UnsafePointer<CChar>?) -> Void
    let onError: @convention(c) (UnsafeMutableRawPointer?, UnsafePointer<CChar>?) -> Void
    let shouldStop: @convention(c) (UnsafeMutableRawPointer?) -> Int32
    let ctx: UnsafeMutableRawPointer?

    init(
        onText: @convention(c) (UnsafeMutableRawPointer?, UnsafePointer<CChar>?) -> Void,
        onError: @convention(c) (UnsafeMutableRawPointer?, UnsafePointer<CChar>?) -> Void,
        shouldStop: @convention(c) (UnsafeMutableRawPointer?) -> Int32,
        ctx: UnsafeMutableRawPointer?
    ) {
        self.onText = onText
        self.onError = onError
        self.shouldStop = shouldStop
        self.ctx = ctx
    }

    func emit(_ text: String) {
        text.withCString { onText(ctx, $0) }
    }

    func fail(_ text: String) {
        text.withCString { onError(ctx, $0) }
    }

    func stopped() -> Bool {
        shouldStop(ctx) != 0
    }
}

#if canImport(FoundationModels)
@available(macOS 26.0, *)
private enum AnyrFm {
    static func status() -> Int32 {
        switch SystemLanguageModel.default.availability {
        case .available:
            return 1
        default:
            return 0
        }
    }

    static func generate(
        instructions: String,
        prompt: String,
        stream: Bool,
        callbacks: Callbacks
    ) async -> Int32 {
        guard case .available = SystemLanguageModel.default.availability else {
            callbacks.fail("system model is unavailable")
            return 1
        }
        if prompt.isEmpty {
            callbacks.fail("the chat request has no user message")
            return 1
        }
        if callbacks.stopped() {
            return 2
        }
        let session = instructions.isEmpty
            ? LanguageModelSession()
            : LanguageModelSession(instructions: instructions)
        do {
            if stream {
                for try await snapshot in session.streamResponse(to: prompt) {
                    if callbacks.stopped() || Task.isCancelled {
                        return 2
                    }
                    callbacks.emit(snapshot.content)
                }
            } else {
                let response = try await session.respond(to: prompt)
                if callbacks.stopped() || Task.isCancelled {
                    return 2
                }
                callbacks.emit(response.content)
            }
            return 0
        } catch is CancellationError {
            return 2
        } catch {
            if callbacks.stopped() || Task.isCancelled {
                return 2
            }
            callbacks.fail(error.localizedDescription)
            return 1
        }
    }
}
#endif
