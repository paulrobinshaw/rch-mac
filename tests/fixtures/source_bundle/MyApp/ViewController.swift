import UIKit

class ViewController: UIViewController {
    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .systemBackground
    }

    /// A simple pure function for testing
    func add(_ a: Int, _ b: Int) -> Int {
        return a + b
    }
}
