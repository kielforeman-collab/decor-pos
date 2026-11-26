import { useRef, useState } from 'react'

/** Touch-friendly signature pad. Draw with finger/stylus/mouse;
 * exports a PNG data URL (the base64 half is sent as signature_b64). */
export default function SignaturePad({
  onChange,
}: {
  onChange: (dataUrl: string | null) => void
}) {
  const canvasRef = useRef<HTMLCanvasElement>(null)
  const drawingRef = useRef(false)
  const [hasInk, setHasInk] = useState(false)

  function pos(e: React.PointerEvent): { x: number; y: number } {
    const canvas = canvasRef.current!
    const rect = canvas.getBoundingClientRect()
    const scaleX = canvas.width / rect.width
    const scaleY = canvas.height / rect.height
    return { x: (e.clientX - rect.left) * scaleX, y: (e.clientY - rect.top) * scaleY }
  }

  function start(e: React.PointerEvent) {
    e.preventDefault()
    const canvas = canvasRef.current!
    canvas.setPointerCapture(e.pointerId)
    drawingRef.current = true
    const { x, y } = pos(e)
    const ctx = canvas.getContext('2d')!
    ctx.beginPath()
    ctx.moveTo(x, y)
  }

  function move(e: React.PointerEvent) {
    if (!drawingRef.current) return
    e.preventDefault()
    const canvas = canvasRef.current!
    const { x, y } = pos(e)
    const ctx = canvas.getContext('2d')!
    ctx.lineWidth = 2.5
    ctx.lineCap = 'round'
    ctx.strokeStyle = '#111'
    ctx.lineTo(x, y)
    ctx.stroke()
    if (!hasInk) setHasInk(true)
  }

  function end() {
    if (!drawingRef.current) return
    drawingRef.current = false
    onChange(canvasRef.current!.toDataURL('image/png'))
  }

  function clear() {
    const canvas = canvasRef.current!
    const ctx = canvas.getContext('2d')!
    ctx.clearRect(0, 0, canvas.width, canvas.height)
    setHasInk(false)
    onChange(null)
  }

  return (
    <div>
      <canvas
        ref={canvasRef}
        className="sig-pad"
        width={640}
        height={220}
        onPointerDown={start}
        onPointerMove={move}
        onPointerUp={end}
        onPointerCancel={end}
        aria-label="Signature pad — sign with your finger or stylus"
      />
      <div className="sig-actions">
        <button type="button" className="btn secondary small" onClick={clear} disabled={!hasInk}>
          Clear signature
        </button>
        {!hasInk && <span className="muted">Sign above to capture a signature.</span>}
      </div>
    </div>
  )
}
