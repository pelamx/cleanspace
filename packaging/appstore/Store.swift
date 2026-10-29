// StoreKit 2 bridge for the Mac App Store build (linked by build.rs, called from src/store.rs).
// Every call answers through a C callback on a background thread.
import Foundation
import StoreKit

public typealias StatusCallback = @convention(c) (Int64) -> Void
public typealias PricesCallback = @convention(c) (UnsafePointer<CChar>?, UnsafePointer<CChar>?) -> Void
/// 0 bought, 1 cancelled, 2 pending (e.g. Ask to Buy), 3 failed
public typealias BuyCallback = @convention(c) (Int32) -> Void

/// End of the latest active subscription period (unix seconds), 0 = none.
func proUntil() async -> Int64 {
    var until: Int64 = 0
    for await result in Transaction.currentEntitlements {
        if case .verified(let t) = result, t.revocationDate == nil, let end = t.expirationDate {
            until = max(until, Int64(end.timeIntervalSince1970))
        }
    }
    return until
}

/// Reports the status now and after every change: renewals, refunds, purchases made on another Mac.
@_cdecl("dm_store_start")
public func dmStoreStart(_ cb: StatusCallback) {
    Task.detached {
        cb(await proUntil())
        for await update in Transaction.updates {
            if case .verified(let t) = update { await t.finish() }
            cb(await proUntil())
        }
    }
}

/// "Restore purchases": syncs with the App Store (may ask the user to sign in).
@_cdecl("dm_store_restore")
public func dmStoreRestore(_ cb: StatusCallback) {
    Task.detached {
        try? await AppStore.sync()
        cb(await proUntil())
    }
}

/// Localized prices of the two plans, e.g. "$4.99" and "$29.99"; nil when the App Store has no answer.
@_cdecl("dm_store_prices")
public func dmStorePrices(_ monthly: UnsafePointer<CChar>, _ yearly: UnsafePointer<CChar>, _ cb: PricesCallback) {
    let ids = [String(cString: monthly), String(cString: yearly)]
    Task.detached {
        let products = (try? await Product.products(for: ids)) ?? []
        let price = { (id: String) in products.first { $0.id == id }?.displayPrice }
        let (m, y) = (price(ids[0]), price(ids[1]))
        m.withOptionalCString { mp in y.withOptionalCString { yp in cb(mp, yp) } }
    }
}

@_cdecl("dm_store_buy")
public func dmStoreBuy(_ id: UnsafePointer<CChar>, _ cb: BuyCallback, _ status: StatusCallback) {
    let id = String(cString: id)
    Task.detached {
        do {
            guard let product = try await Product.products(for: [id]).first else { return cb(3) }
            switch try await product.purchase() {
            case .success(.verified(let t)):
                await t.finish()
                cb(0)
                status(await proUntil())
            case .success(.unverified):
                cb(3)
            case .userCancelled:
                cb(1)
            case .pending:
                cb(2)
            @unknown default:
                cb(3)
            }
        } catch {
            cb(3)
        }
    }
}

extension Optional where Wrapped == String {
    func withOptionalCString<R>(_ body: (UnsafePointer<CChar>?) -> R) -> R {
        switch self {
        case .some(let s): return s.withCString { body($0) }
        case .none: return body(nil)
        }
    }
}
