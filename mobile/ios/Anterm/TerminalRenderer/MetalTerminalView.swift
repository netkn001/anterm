import CoreGraphics
import CoreText
import Metal
import MetalKit
import SwiftUI
import UIKit
import simd

/// Decoded header matching `anterm-client-core/src/frame.rs` (24 bytes, little-endian).
struct DecodedFrameHeader {
    let revision: UInt64
    let cols: UInt16
    let rows: UInt16
    let cursorCol: UInt16
    let cursorRow: UInt16
    let cursorShape: UInt8
    let flags: UInt8
    let dirtyRowCount: UInt16
}

struct DecodedPackedCell {
    let codepoint: UInt32
    let fgRgba: UInt32
    let bgRgba: UInt32
    let style: UInt16
}

private struct MetalCellInstance {
    var origin: SIMD2<Float>
    var size: SIMD2<Float>
    var bgColor: SIMD4<Float>
    var fgColor: SIMD4<Float>
    var uvRect: SIMD4<Float>
    var flags: UInt32
    var _pad0: UInt32 = 0
    var _pad1: UInt32 = 0
    var _pad2: UInt32 = 0
}

private struct ViewportUniforms {
    var viewportSize: SIMD2<Float>
    var panOffset: SIMD2<Float>
    var zoomScale: Float
    var _pad0: Float = 0
    var _pad1: Float = 0
    var _pad2: Float = 0
}

/// Native 120Hz Metal terminal renderer consuming Rust's packed 16-byte/cell frame buffer,
/// with smooth pinch-to-zoom Peek scaling and pan scrollback without altering PC PTY columns.
struct MetalTerminalView: UIViewRepresentable {
    let frameBytes: Data
    var onScroll: ((Int32) -> Void)?

    func makeCoordinator() -> MetalTerminalRenderer {
        MetalTerminalRenderer(onScroll: onScroll)
    }

    func makeUIView(context: Context) -> MTKView {
        let view = MTKView(frame: .zero, device: MTLCreateSystemDefaultDevice())
        view.enableSetNeedsDisplay = true
        view.isPaused = true
        view.framebufferOnly = true
        view.preferredFramesPerSecond = 120
        view.clearColor = MTLClearColor(red: 0.051, green: 0.067, blue: 0.090, alpha: 1.0)
        view.delegate = context.coordinator

        let pinch = UIPinchGestureRecognizer(
            target: context.coordinator,
            action: #selector(MetalTerminalRenderer.handlePinch(_:))
        )
        let pan = UIPanGestureRecognizer(
            target: context.coordinator,
            action: #selector(MetalTerminalRenderer.handlePan(_:))
        )
        let doubleTap = UITapGestureRecognizer(
            target: context.coordinator,
            action: #selector(MetalTerminalRenderer.handleDoubleTap(_:))
        )
        doubleTap.numberOfTapsRequired = 2
        view.addGestureRecognizer(pinch)
        view.addGestureRecognizer(pan)
        view.addGestureRecognizer(doubleTap)

        context.coordinator.attach(view: view)
        context.coordinator.applyFrame(frameBytes, view: view)
        return view
    }

    func updateUIView(_ uiView: MTKView, context: Context) {
        context.coordinator.onScroll = onScroll
        context.coordinator.applyFrame(frameBytes, view: uiView)
    }
}

private let terminalMetalShaderSource = """
#include <metal_stdlib>
using namespace metal;

struct CellInstance {
    float2 origin;
    float2 size;
    float4 bgColor;
    float4 fgColor;
    float4 uvRect;
    uint flags;
};

struct ViewportUniforms {
    float2 viewportSize;
    float2 panOffset;
    float zoomScale;
    float _pad0;
    float _pad1;
    float _pad2;
};

struct VertexOut {
    float4 position [[position]];
    float2 uv;
    float4 bgColor;
    float4 fgColor;
    uint flags;
};

constant float2 kQuadVertices[6] = {
    float2(0.0, 0.0),
    float2(1.0, 0.0),
    float2(0.0, 1.0),
    float2(1.0, 0.0),
    float2(1.0, 1.0),
    float2(0.0, 1.0),
};

vertex VertexOut terminal_cell_vertex(
    uint vertexId [[vertex_id]],
    uint instanceId [[instance_id]],
    constant CellInstance *instances [[buffer(0)]],
    constant ViewportUniforms &uniforms [[buffer(1)]]
) {
    CellInstance cell = instances[instanceId];
    float2 corner = kQuadVertices[vertexId];
    float2 pixelPos = (cell.origin + corner * cell.size) * uniforms.zoomScale + uniforms.panOffset;

    float2 ndc = (pixelPos / max(uniforms.viewportSize, float2(1.0))) * 2.0 - 1.0;
    ndc.y = -ndc.y;

    VertexOut out;
    out.position = float4(ndc, 0.0, 1.0);
    out.uv = cell.uvRect.xy + corner * cell.uvRect.zw;
    out.bgColor = cell.bgColor;
    out.fgColor = cell.fgColor;
    out.flags = cell.flags;
    return out;
}

fragment float4 terminal_cell_fragment(
    VertexOut in [[stage_in]],
    texture2d<float> glyphAtlas [[texture(0)]],
    sampler glyphSampler [[sampler(0)]]
) {
    float alpha = 0.0;
    if ((in.flags & 1u) != 0u) {
        alpha = glyphAtlas.sample(glyphSampler, in.uv).r;
    }
    return mix(in.bgColor, in.fgColor, alpha);
}
"""

final class MetalTerminalRenderer: NSObject, MTKViewDelegate {
    var onScroll: ((Int32) -> Void)?

    private weak var hostView: MTKView?
    private var device: MTLDevice?
    private var commandQueue: MTLCommandQueue?
    private var pipelineState: MTLRenderPipelineState?
    private var samplerState: MTLSamplerState?
    private var atlasTexture: MTLTexture?

    private var cols: Int = 80
    private var rows: Int = 24
    private var cursorCol: Int = 0
    private var cursorRow: Int = 0
    private var cursorVisible: Bool = true
    private var grid: [[DecodedPackedCell]] = []

    private var zoomScale: Float = 1.0
    private var panOffset: SIMD2<Float> = .zero
    private var scrollAccumulator: CGFloat = 0

    private let atlasDim: Int = 1024
    private let cellPxW: Int = 16
    private let cellPxH: Int = 32
    private var glyphMap: [UInt32: SIMD4<Float>] = [:]
    private var nextAtlasSlot: Int = 0

    init(onScroll: ((Int32) -> Void)?) {
        self.onScroll = onScroll
    }

    @objc func handlePinch(_ recognizer: UIPinchGestureRecognizer) {
        let next = zoomScale * Float(recognizer.scale)
        zoomScale = min(max(next, 0.75), 3.0)
        if zoomScale <= 1.02 {
            panOffset = .zero
        }
        recognizer.scale = 1.0
        hostView?.setNeedsDisplay()
    }

    @objc func handlePan(_ recognizer: UIPanGestureRecognizer) {
        guard let view = hostView else { return }
        let translation = recognizer.translation(in: view)
        let scaleFactor = Float(view.contentScaleFactor)
        if zoomScale > 1.05 {
            panOffset.x = min(0, panOffset.x + Float(translation.x) * scaleFactor)
            panOffset.y = min(0, panOffset.y + Float(translation.y) * scaleFactor)
            recognizer.setTranslation(.zero, in: view)
            view.setNeedsDisplay()
        } else {
            scrollAccumulator += translation.y
            let lineStep: CGFloat = 18
            let lines = Int32(scrollAccumulator / lineStep)
            if lines != 0 {
                scrollAccumulator -= CGFloat(lines) * lineStep
                onScroll?(lines)
            }
            recognizer.setTranslation(.zero, in: view)
        }
    }

    @objc func handleDoubleTap(_ recognizer: UITapGestureRecognizer) {
        zoomScale = 1.0
        panOffset = .zero
        hostView?.setNeedsDisplay()
    }

    func attach(view: MTKView) {
        self.hostView = view
        guard let device = view.device else { return }
        self.device = device
        self.commandQueue = device.makeCommandQueue()

        if let library = (try? device.makeLibrary(source: terminalMetalShaderSource, options: nil))
            ?? device.makeDefaultLibrary(),
           let vert = library.makeFunction(name: "terminal_cell_vertex"),
           let frag = library.makeFunction(name: "terminal_cell_fragment") {
            let desc = MTLRenderPipelineDescriptor()
            desc.vertexFunction = vert
            desc.fragmentFunction = frag
            desc.colorAttachments[0].pixelFormat = view.colorPixelFormat
            self.pipelineState = try? device.makeRenderPipelineState(descriptor: desc)
        }

        let sampDesc = MTLSamplerDescriptor()
        sampDesc.minFilter = .linear
        sampDesc.magFilter = .linear
        self.samplerState = device.makeSamplerState(descriptor: sampDesc)

        let texDesc = MTLTextureDescriptor.texture2DDescriptor(
            pixelFormat: .r8Unorm,
            width: atlasDim,
            height: atlasDim,
            mipmapped: false
        )
        texDesc.usage = [.shaderRead]
        self.atlasTexture = device.makeTexture(descriptor: texDesc)
    }

    func applyFrame(_ data: Data, view: MTKView) {
        guard data.count >= 24 else { return }
        data.withUnsafeBytes { raw in
            guard let base = raw.baseAddress else { return }
            let revision = base.loadUnaligned(fromByteOffset: 0, as: UInt64.self)
            _ = revision
            let newCols = Int(base.loadUnaligned(fromByteOffset: 8, as: UInt16.self))
            let newRows = Int(base.loadUnaligned(fromByteOffset: 10, as: UInt16.self))
            let cCol = Int(base.loadUnaligned(fromByteOffset: 12, as: UInt16.self))
            let cRow = Int(base.loadUnaligned(fromByteOffset: 14, as: UInt16.self))
            let flags = base.loadUnaligned(fromByteOffset: 17, as: UInt8.self)
            let dirtyRows = Int(base.loadUnaligned(fromByteOffset: 18, as: UInt16.self))

            guard newCols > 0, newRows > 0 else { return }
            if newCols != self.cols || newRows != self.rows || self.grid.count != newRows {
                self.cols = newCols
                self.rows = newRows
                let blank = DecodedPackedCell(codepoint: 32, fgRgba: 0xE6EDF3FF, bgRgba: 0x0D1117FF, style: 0)
                self.grid = Array(repeating: Array(repeating: blank, count: newCols), count: newRows)
            }
            self.cursorCol = cCol
            self.cursorRow = cRow
            self.cursorVisible = (flags & 0x02) != 0

            var offset = 24
            for _ in 0..<dirtyRows {
                guard offset + 4 <= data.count else { break }
                let rowIdx = Int(base.loadUnaligned(fromByteOffset: offset, as: UInt16.self))
                let cellCount = Int(base.loadUnaligned(fromByteOffset: offset + 2, as: UInt16.self))
                offset += 4
                let rowBytes = cellCount * 16
                guard offset + rowBytes <= data.count else { break }
                if rowIdx < self.rows {
                    let limit = min(cellCount, self.cols)
                    for col in 0..<limit {
                        let cOff = offset + col * 16
                        let cp = base.loadUnaligned(fromByteOffset: cOff, as: UInt32.self)
                        let fg = base.loadUnaligned(fromByteOffset: cOff + 4, as: UInt32.self)
                        let bg = base.loadUnaligned(fromByteOffset: cOff + 8, as: UInt32.self)
                        let style = base.loadUnaligned(fromByteOffset: cOff + 12, as: UInt16.self)
                        self.grid[rowIdx][col] = DecodedPackedCell(
                            codepoint: cp,
                            fgRgba: fg,
                            bgRgba: bg,
                            style: style
                        )
                    }
                }
                offset += rowBytes
            }
        }
        view.setNeedsDisplay()
    }

    func mtkView(_ view: MTKView, drawableSizeWillChange size: CGSize) {
        view.setNeedsDisplay()
    }

    func draw(in view: MTKView) {
        guard let device,
              let queue = commandQueue,
              let pipeline = pipelineState,
              let atlas = atlasTexture,
              let sampler = samplerState,
              let passDesc = view.currentRenderPassDescriptor,
              let drawable = view.currentDrawable else {
            return
        }

        let drawableW = Float(view.drawableSize.width)
        let drawableH = Float(view.drawableSize.height)
        guard drawableW > 0, drawableH > 0, cols > 0, rows > 0 else { return }

        let cellW = drawableW / Float(cols)
        let cellH = min(drawableH / Float(rows), cellW * 2.15)

        var instances: [MetalCellInstance] = []
        instances.reserveCapacity(cols * rows)

        for r in 0..<min(rows, grid.count) {
            let row = grid[r]
            for c in 0..<min(cols, row.count) {
                let cell = row[c]
                var fg = unpackRgba(cell.fgRgba)
                var bg = unpackRgba(cell.bgRgba)
                if cursorVisible && r == cursorRow && c == cursorCol {
                    swap(&fg, &bg)
                }
                let uv: SIMD4<Float>
                let hasGlyph: UInt32
                if cell.codepoint > 32, let rect = glyphUV(for: cell.codepoint, texture: atlas) {
                    uv = rect
                    hasGlyph = 1
                } else {
                    uv = .zero
                    hasGlyph = 0
                }
                instances.append(
                    MetalCellInstance(
                        origin: SIMD2<Float>(Float(c) * cellW, Float(r) * cellH),
                        size: SIMD2<Float>(cellW, cellH),
                        bgColor: bg,
                        fgColor: fg,
                        uvRect: uv,
                        flags: hasGlyph
                    )
                )
            }
        }

        guard !instances.isEmpty,
              let instanceBuf = device.makeBuffer(
                  bytes: instances,
                  length: MemoryLayout<MetalCellInstance>.stride * instances.count,
                  options: .storageModeShared
              ) else {
            return
        }

        var uniforms = ViewportUniforms(
            viewportSize: SIMD2<Float>(drawableW, drawableH),
            panOffset: panOffset,
            zoomScale: zoomScale
        )
        guard let cmdBuf = queue.makeCommandBuffer(),
              let encoder = cmdBuf.makeRenderCommandEncoder(descriptor: passDesc) else {
            return
        }

        encoder.setRenderPipelineState(pipeline)
        encoder.setVertexBuffer(instanceBuf, offset: 0, index: 0)
        encoder.setVertexBytes(&uniforms, length: MemoryLayout<ViewportUniforms>.stride, index: 1)
        encoder.setFragmentTexture(atlas, index: 0)
        encoder.setFragmentSamplerState(sampler, index: 0)
        encoder.drawPrimitives(
            type: .triangle,
            vertexStart: 0,
            vertexCount: 6,
            instanceCount: instances.count
        )
        encoder.endEncoding()
        cmdBuf.present(drawable)
        cmdBuf.commit()
    }

    private func glyphUV(for codepoint: UInt32, texture: MTLTexture) -> SIMD4<Float>? {
        if let cached = glyphMap[codepoint] {
            return cached
        }
        guard let scalar = UnicodeScalar(codepoint) else { return nil }
        let slotsPerRow = atlasDim / cellPxW
        let maxSlots = slotsPerRow * (atlasDim / cellPxH)
        guard nextAtlasSlot < maxSlots else { return nil }

        let slotX = (nextAtlasSlot % slotsPerRow) * cellPxW
        let slotY = (nextAtlasSlot / slotsPerRow) * cellPxH
        nextAtlasSlot += 1

        var bitmap = [UInt8](repeating: 0, count: cellPxW * cellPxH)
        bitmap.withUnsafeMutableBytes { ptr in
            guard let base = ptr.baseAddress,
                  let ctx = CGContext(
                      data: base,
                      width: cellPxW,
                      height: cellPxH,
                      bitsPerComponent: 8,
                      bytesPerRow: cellPxW,
                      space: CGColorSpaceCreateDeviceGray(),
                      bitmapInfo: CGImageAlphaInfo.none.rawValue
                  ) else {
                return
            }
            ctx.setFillColor(gray: 0, alpha: 1)
            ctx.fill(CGRect(x: 0, y: 0, width: cellPxW, height: cellPxH))
            ctx.setFillColor(gray: 1, alpha: 1)

            // Crisp pixel-aligned lines for common box-drawing characters so TUI borders never fracture.
            if drawBoxDrawingIfMatched(codepoint: codepoint, ctx: ctx) {
                return
            }

            let font = CTFontCreateWithName("Menlo-Regular" as CFString, 22, nil)
            let attrs: [CFString: Any] = [
                kCTFontAttributeName: font,
                kCTForegroundColorFromContextAttributeName: true
            ]
            let attrStr = CFAttributedStringCreate(nil, String(scalar) as CFString, attrs as CFDictionary)!
            let line = CTLineCreateWithAttributedString(attrStr)
            ctx.textPosition = CGPoint(x: 1, y: 7)
            CTLineDraw(line, ctx)
        }

        let region = MTLRegionMake2D(slotX, slotY, cellPxW, cellPxH)
        texture.replace(region: region, mipmapLevel: 0, withBytes: bitmap, bytesPerRow: cellPxW)

        let inv = 1.0 / Float(atlasDim)
        let uv = SIMD4<Float>(
            Float(slotX) * inv,
            Float(slotY) * inv,
            Float(cellPxW) * inv,
            Float(cellPxH) * inv
        )
        glyphMap[codepoint] = uv
        return uv
    }

    private func drawBoxDrawingIfMatched(codepoint: UInt32, ctx: CGContext) -> Bool {
        let w = CGFloat(cellPxW)
        let h = CGFloat(cellPxH)
        let mx = w * 0.5
        let my = h * 0.5
        let t: CGFloat = 2.0

        switch codepoint {
        case 0x2500, 0x2501: // ─ ━
            ctx.fill(CGRect(x: 0, y: my - t * 0.5, width: w, height: t))
            return true
        case 0x2502, 0x2503: // │ ┃
            ctx.fill(CGRect(x: mx - t * 0.5, y: 0, width: t, height: h))
            return true
        case 0x250C, 0x256D: // ┌ ╭
            ctx.fill(CGRect(x: mx - t * 0.5, y: my - t * 0.5, width: w - mx + t * 0.5, height: t))
            ctx.fill(CGRect(x: mx - t * 0.5, y: 0, width: t, height: my + t * 0.5))
            return true
        case 0x2510, 0x256E: // ┐ ╮
            ctx.fill(CGRect(x: 0, y: my - t * 0.5, width: mx + t * 0.5, height: t))
            ctx.fill(CGRect(x: mx - t * 0.5, y: 0, width: t, height: my + t * 0.5))
            return true
        case 0x2514, 0x2570: // └ ╰
            ctx.fill(CGRect(x: mx - t * 0.5, y: my - t * 0.5, width: w - mx + t * 0.5, height: t))
            ctx.fill(CGRect(x: mx - t * 0.5, y: my - t * 0.5, width: t, height: h - my + t * 0.5))
            return true
        case 0x2518, 0x256F: // ┘ ╯
            ctx.fill(CGRect(x: 0, y: my - t * 0.5, width: mx + t * 0.5, height: t))
            ctx.fill(CGRect(x: mx - t * 0.5, y: my - t * 0.5, width: t, height: h - my + t * 0.5))
            return true
        case 0x253C: // ┼
            ctx.fill(CGRect(x: 0, y: my - t * 0.5, width: w, height: t))
            ctx.fill(CGRect(x: mx - t * 0.5, y: 0, width: t, height: h))
            return true
        default:
            return false
        }
    }

    private func unpackRgba(_ packed: UInt32) -> SIMD4<Float> {
        let r = Float((packed >> 24) & 0xFF) / 255.0
        let g = Float((packed >> 16) & 0xFF) / 255.0
        let b = Float((packed >> 8) & 0xFF) / 255.0
        let a = Float(packed & 0xFF) / 255.0
        return SIMD4<Float>(r, g, b, a)
    }
}
