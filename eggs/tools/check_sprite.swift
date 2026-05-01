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

    func pixel(x: Int, y: Int) -> (r: UInt8, g: UInt8, b: UInt8, a: UInt8) {
        let i = (y * width + x) * 4
        return (data[i], data[i + 1], data[i + 2], data[i + 3])
    }
}

func loadImage(_ path: String) -> RGBAImage {
    let url = URL(fileURLWithPath: path)
    let source = CGImageSourceCreateWithURL(url as CFURL, nil)!
    return RGBAImage(cgImage: CGImageSourceCreateImageAtIndex(source, 0, nil)!)
}

func isDarkNeutral(_ p: (r: UInt8, g: UInt8, b: UInt8, a: UInt8)) -> Bool {
    let minChannel = min(Int(p.r), Int(p.g), Int(p.b))
    let maxChannel = max(Int(p.r), Int(p.g), Int(p.b))
    let brightness = (Int(p.r) + Int(p.g) + Int(p.b)) / 3
    return p.a > 16 && brightness <= 185 && maxChannel - minChannel <= 45
}

func isWhiteBackground(_ p: (r: UInt8, g: UInt8, b: UInt8, a: UInt8)) -> Bool {
    let minChannel = min(Int(p.r), Int(p.g), Int(p.b))
    let maxChannel = max(Int(p.r), Int(p.g), Int(p.b))
    let brightness = (Int(p.r) + Int(p.g) + Int(p.b)) / 3
    return p.a > 16 && brightness >= 232 && maxChannel - minChannel <= 35
}

let paths = CommandLine.arguments.dropFirst()
for path in paths {
    let image = loadImage(String(path))
    var edgeDark = 0
    var edgeOpaque = 0
    var white = 0
    var opaque = 0
    for y in 0..<image.height {
        for x in 0..<image.width {
            let p = image.pixel(x: x, y: y)
            if p.a > 16 {
                opaque += 1
                if min(x, y, image.width - 1 - x, image.height - 1 - y) < 20 {
                    edgeOpaque += 1
                }
            }
            if isWhiteBackground(p) {
                white += 1
            }
            if min(x, y, image.width - 1 - x, image.height - 1 - y) < 20 && isDarkNeutral(p) {
                edgeDark += 1
            }
        }
    }
    print("\(URL(fileURLWithPath: String(path)).lastPathComponent): opaque=\(opaque) whiteLike=\(white) edgeOpaque=\(edgeOpaque) edgeDark=\(edgeDark)")
}
