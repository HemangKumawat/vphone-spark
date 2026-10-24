// vphone-linux overlay (new file): Linux Foundation ignores URLSessionTask.delegate, so a task
// whose callbacks go to its own delegate (VPhoneIPSWCache.transfer) never hears from URLSession
// and its download waits forever. Sessions that run such tasks are created with this router as
// their delegate; it passes each task's callbacks to the delegate registered for it.

import Foundation
#if canImport(FoundationNetworking)
import FoundationNetworking
#endif

public final class VPhoneTransferRouter: NSObject, URLSessionDataDelegate, @unchecked Sendable {
    public static let shared = VPhoneTransferRouter()

    private let lock = NSLock()
    private var delegates: [ObjectIdentifier: URLSessionDataDelegate] = [:]

    /// A session whose tasks' delegates are honoured.
    public static func session(configuration: URLSessionConfiguration) -> URLSession {
        URLSession(configuration: configuration, delegate: shared, delegateQueue: nil)
    }

    /// Call before the task resumes.
    func register(_ task: URLSessionTask, _ delegate: URLSessionDataDelegate) {
        lock.lock(); defer { lock.unlock() }
        delegates[ObjectIdentifier(task)] = delegate
    }

    private func delegate(for task: URLSessionTask, remove: Bool = false) -> URLSessionDataDelegate? {
        lock.lock(); defer { lock.unlock() }
        return remove ? delegates.removeValue(forKey: ObjectIdentifier(task)) : delegates[ObjectIdentifier(task)]
    }

    public func urlSession(
        _ session: URLSession,
        dataTask: URLSessionDataTask,
        didReceive response: URLResponse,
        completionHandler: @escaping @Sendable (URLSession.ResponseDisposition) -> Void,
    ) {
        guard let delegate = delegate(for: dataTask) else { return completionHandler(.cancel) }
        delegate.urlSession(session, dataTask: dataTask, didReceive: response, completionHandler: completionHandler)
    }

    public func urlSession(_ session: URLSession, dataTask: URLSessionDataTask, didReceive data: Data) {
        delegate(for: dataTask)?.urlSession(session, dataTask: dataTask, didReceive: data)
    }

    public func urlSession(_ session: URLSession, task: URLSessionTask, didCompleteWithError error: Swift.Error?) {
        delegate(for: task, remove: true)?.urlSession(session, task: task, didCompleteWithError: error)
    }
}
