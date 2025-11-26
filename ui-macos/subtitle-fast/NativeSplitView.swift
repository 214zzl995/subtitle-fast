import SwiftUI
import AppKit

struct NativeSplitView<Left: View, Right: View>: NSViewControllerRepresentable {
    let minLeftWidth: CGFloat
    let minRightWidth: CGFloat
    let initialRightWidth: CGFloat
    @ViewBuilder let left: () -> Left
    @ViewBuilder let right: () -> Right

    func makeNSViewController(context: Context) -> SplitHostController<Left, Right> {
        SplitHostController(
            minLeftWidth: minLeftWidth,
            minRightWidth: minRightWidth,
            initialRightWidth: initialRightWidth,
            left: left(),
            right: right()
        )
    }

    func updateNSViewController(_ controller: SplitHostController<Left, Right>, context: Context) {
        controller.update(left: left(), right: right())
    }
}

final class SplitHostController<Left: View, Right: View>: NSSplitViewController {
    private var leftHost: NSHostingController<Left>
    private var rightHost: NSHostingController<Right>
    private let minLeftWidth: CGFloat
    private let minRightWidth: CGFloat
    private let initialRightWidth: CGFloat
    private var didSetInitial = false

    init(
        minLeftWidth: CGFloat,
        minRightWidth: CGFloat,
        initialRightWidth: CGFloat,
        left: Left,
        right: Right
    ) {
        self.leftHost = NSHostingController(rootView: left)
        self.rightHost = NSHostingController(rootView: right)
        self.minLeftWidth = minLeftWidth
        self.minRightWidth = minRightWidth
        self.initialRightWidth = initialRightWidth
        super.init(nibName: nil, bundle: nil)

        splitView.isVertical = true

        let leftItem = NSSplitViewItem(viewController: leftHost)
        leftItem.minimumThickness = minLeftWidth
        leftItem.holdingPriority = NSLayoutConstraint.Priority.defaultLow

        let rightItem = NSSplitViewItem(viewController: rightHost)
        rightItem.minimumThickness = minRightWidth
        rightItem.holdingPriority = NSLayoutConstraint.Priority.defaultLow

        addSplitViewItem(leftItem)
        addSplitViewItem(rightItem)
    }

    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    override func viewDidLayout() {
        super.viewDidLayout()
        guard !didSetInitial else { return }
        let total = splitView.bounds.width
        let target = max(total - initialRightWidth, minLeftWidth)
        splitView.setPosition(target, ofDividerAt: 0)
        didSetInitial = true
    }

    func update(left: Left, right: Right) {
        leftHost.rootView = left
        rightHost.rootView = right
    }
}
