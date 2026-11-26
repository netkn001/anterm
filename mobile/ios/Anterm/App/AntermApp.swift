import SwiftUI

@main
struct AntermApp: App {
    @StateObject private var store = MobileWorkbenchStore()

    var body: some Scene {
        WindowGroup {
            NavigationStack {
                WorkbenchView()
            }
            .environmentObject(store)
            .preferredColorScheme(.dark)
            .tint(Color(red: 0.36, green: 0.68, blue: 1.0))
        }
    }
}
