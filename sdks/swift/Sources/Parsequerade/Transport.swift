import Foundation
import ParsequeradeCore

/// A trusted transport receives signed requests and must honor their timeout, avoid
/// redirects and retries, and return a bounded response body. Omit it to use Rust HTTP.
public typealias AppleTransport = @Sendable (URLRequest) async throws -> (Data, HTTPURLResponse)

final class OperationCancellation: @unchecked Sendable {
    let id: UInt64
    private let lock = NSLock()
    private var cancelled = false
    private var interrupt: (@Sendable () -> Void)?
    init(_ id: UInt64) { self.id = id }
    func install(_ interrupt: @escaping @Sendable () -> Void) {
        lock.lock()
        if cancelled { lock.unlock(); interrupt() }
        else { self.interrupt = interrupt; lock.unlock() }
    }
    func clear() { lock.lock(); interrupt = nil; lock.unlock() }
    func cancel() {
        lock.lock(); cancelled = true; let interrupt = interrupt; lock.unlock()
        pq_cancel(id)
        interrupt?()
    }
}

// A custom callback may ignore cancellation. Resume the SDK caller promptly and
// discard any late response without holding an operation or a foreign pointer.
private final class CallbackWaiter<Value>: @unchecked Sendable {
    private let lock = NSLock()
    private var continuation: CheckedContinuation<Value, Error>?
    private var outcome: Result<Value, Error>?
    private var workers: [Task<Void, Never>] = []
    func install(_ continuation: CheckedContinuation<Value, Error>) {
        lock.lock()
        if let outcome { lock.unlock(); continuation.resume(with: outcome) }
        else { self.continuation = continuation; lock.unlock() }
    }
    var isPending: Bool { lock.lock(); defer { lock.unlock() }; return outcome == nil }
    func attach(_ worker: Task<Void, Never>) {
        lock.lock()
        if outcome != nil { lock.unlock(); worker.cancel() }
        else { workers.append(worker); lock.unlock() }
    }
    func finish(_ result: Result<Value, Error>) {
        lock.lock()
        guard outcome == nil else { lock.unlock(); return }
        outcome = result
        let continuation = continuation
        self.continuation = nil
        let workers = workers
        self.workers = []
        lock.unlock()
        for worker in workers { worker.cancel() }
        continuation?.resume(with: result)
    }
}

enum CallbackAborted: Error { case cancelled }

func invokeTransport(_ transport: @escaping AppleTransport, _ request: URLRequest,
                     cancellation: OperationCancellation, deadline: TimeInterval) async throws -> (Data, HTTPURLResponse) {
    let waiter = CallbackWaiter<(Data, HTTPURLResponse)>()
    cancellation.install { waiter.finish(.failure(CallbackAborted.cancelled)) }
    defer { cancellation.clear() }
    return try await withTaskCancellationHandler {
        try await withCheckedThrowingContinuation { continuation in
            waiter.install(continuation)
            guard waiter.isPending else { return }
            waiter.attach(Task {
                guard waiter.isPending, !Task.isCancelled else { return }
                let remaining = deadline - Date().timeIntervalSince1970
                guard remaining > 0 else { waiter.finish(.failure(URLError(.timedOut))); return }
                var bounded = request
                bounded.timeoutInterval = min(request.timeoutInterval, remaining)
                do { waiter.finish(.success(try await transport(bounded))) }
                catch { waiter.finish(.failure(error)) }
            })
            guard waiter.isPending else { return }
            waiter.attach(Task {
                do {
                    try await Task.sleep(nanoseconds: UInt64(max(0, deadline - Date().timeIntervalSince1970) * 1_000_000_000))
                    waiter.finish(.failure(URLError(.timedOut)))
                } catch { /* The transport finished or was cancelled. */ }
            })
        }
    } onCancel: { waiter.finish(.failure(CallbackAborted.cancelled)) }
}

func invokePersistence(_ persist: @escaping @Sendable (ListingImageUploadCheckpoint) async throws -> Void,
                       snapshot: ListingImageUploadCheckpoint, cancellation: OperationCancellation,
                       owner: ListingImageUploadCheckpoint? = nil) async throws {
    let waiter = CallbackWaiter<Void>()
    cancellation.install { waiter.finish(.failure(CallbackAborted.cancelled)) }
    defer { cancellation.clear() }
    try await withTaskCancellationHandler {
        try await withCheckedThrowingContinuation { continuation in
            waiter.install(continuation)
            guard waiter.isPending else { return }
            // Hold the checkpoint guard until the actual callback exits, even if
            // cancellation lets its caller return first. Late durable saves must
            // not race another upload using the same checkpoint object.
            owner?.retainPersistence()
            waiter.attach(Task {
                defer { owner?.releasePersistence() }
                guard waiter.isPending, !Task.isCancelled else { return }
                do { try await persist(snapshot); waiter.finish(.success(())) }
                catch { waiter.finish(.failure(error)) }
            })
        }
    } onCancel: { waiter.finish(.failure(CallbackAborted.cancelled)) }
}
