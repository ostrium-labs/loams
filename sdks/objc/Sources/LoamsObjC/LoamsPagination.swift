import Foundation

#if canImport(ObjectiveC)
@objcMembers
public class LoamsPagination: NSObject {
    public static func iterate(fetcher: @escaping (String) -> [String: Any]?) -> [Any] {
        var items: [Any] = []
        var pageToken = ""

        while true {
            guard let page = fetcher(pageToken) else { break }

            if let pageItems = page["items"] as? [Any] {
                items.append(contentsOf: pageItems)
            }

            guard let next = page["nextPageToken"] as? String,
                  !next.isEmpty,
                  next != pageToken else {
                break
            }
            pageToken = next
        }

        return items
    }
}
#else
public class LoamsPagination: NSObject {
    public static func iterate(fetcher: @escaping (String) -> [String: Any]?) -> [Any] {
        var items: [Any] = []
        var pageToken = ""

        while true {
            guard let page = fetcher(pageToken) else { break }

            if let pageItems = page["items"] as? [Any] {
                items.append(contentsOf: pageItems)
            }

            guard let next = page["nextPageToken"] as? String,
                  !next.isEmpty,
                  next != pageToken else {
                break
            }
            pageToken = next
        }

        return items
    }
}
#endif
