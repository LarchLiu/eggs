import Foundation
import CoreGraphics
import ImageIO

struct RGBAImage {
    let width: Int
    let height: Int
    var data: [UInt8]

    init(cgImage: CGImage) {
        width = cgImage.width
        height = cgImage.height
        data = [UInt8](repeating: 0, count: width * height * 4)
        let context = CGContext(
            data: &data,
            width: width,
            height: height,
            bitsPerComponent: 8,
            bytesPerRow: width * 4,
            space: CGColorSpaceCreateDeviceRGB(),
            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue | CGBitmapInfo.byteOrder32Big.rawValue
        )!
        context.draw(cgImage, in: CGRect(x: 0, y: 0, width: width, height: height))
    }

    func alpha(x: Int, y: Int) -> UInt8 {
        data[(y * width + x) * 4 + 3]
    }
}

func loadImage(_ path: String) -> RGBAImage {
    let url = URL(fileURLWithPath: path)
    let source = CGImageSourceCreateWithURL(url as CFURL, nil)!
    return RGBAImage(cgImage: CGImageSourceCreateImageAtIndex(source, 0, nil)!)
}

for path in CommandLine.arguments.dropFirst() {
    let image = loadImage(path)
    var minX = image.width
    var minY = image.height
    var maxX = -1
    var maxY = -1
    for y in 0..<image.height {
        for x in 0..<image.width {
            if image.alpha(x: x, y: y) > 16 {
                minX = min(minX, x)
                minY = min(minY, y)
                maxX = max(maxX, x)
                maxY = max(maxY, y)
            }
        }
    }
    let name = URL(fileURLWithPath: path).lastPathComponent
    if maxX >= 0 {
        print("\(name): bbox=(\(minX),\(minY))-(\(maxX),\(maxY)) size=\(maxX - minX + 1)x\(maxY - minY + 1)")
    } else {
        print("\(name): empty")
    }
}
