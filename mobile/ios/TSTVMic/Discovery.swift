import Foundation
import Network

/// Finds desktop apps advertising `_tstvmic._tcp` on the local network.
final class Discovery {
    /// Called on the main thread with the current list.
    var onChange: (([DesktopTarget]) -> Void)?
    /// Called on the main thread with whether iOS is blocking local network access.
    var onLocalNetworkDenied: ((Bool) -> Void)?
    private var browser: NWBrowser?

    func start() {
        guard browser == nil else { return }
        let browser = NWBrowser(for: .bonjour(type: Wire.serviceType, domain: nil), using: .tcp)
        browser.browseResultsChangedHandler = { [weak self] results, _ in
            let targets = results.compactMap { result -> DesktopTarget? in
                guard case .service(let name, _, _, _) = result.endpoint else { return nil }
                return .service(name: name, endpoint: result.endpoint)
            }
            .sorted { $0.label < $1.label }
            self?.onChange?(targets)
        }
        browser.stateUpdateHandler = { [weak self] state in
            switch state {
            case .ready:
                self?.onLocalNetworkDenied?(false)
            case .waiting(.dns(StreamClient.dnsPolicyDenied)), .failed(.dns(StreamClient.dnsPolicyDenied)):
                self?.onLocalNetworkDenied?(true)
            default:
                break
            }
        }
        browser.start(queue: .main)
        self.browser = browser
    }

    func stop() {
        browser?.cancel()
        browser = nil
    }
}
