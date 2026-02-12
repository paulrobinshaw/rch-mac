import XCTest
@testable import MyApp

final class MyAppTests: XCTestCase {
    func testAddition() throws {
        let vc = ViewController()
        XCTAssertEqual(vc.add(2, 3), 5)
    }

    func testAdditionWithZero() throws {
        let vc = ViewController()
        XCTAssertEqual(vc.add(0, 5), 5)
        XCTAssertEqual(vc.add(5, 0), 5)
    }

    func testAdditionWithNegative() throws {
        let vc = ViewController()
        XCTAssertEqual(vc.add(-1, 1), 0)
        XCTAssertEqual(vc.add(-5, -3), -8)
    }
}
