import { ImageResponse } from 'next/og'

export const runtime = 'edge'
export const size = { width: 32, height: 32 }
export const contentType = 'image/png'

// Six-blade Lens aperture (matches components/LensLogo.tsx), cyan on dark.
function apertureSvg(px: number): string {
  const BLADES = 6, CX = 12, CY = 12, RO = 10.2, RI = 3.9
  const TW = (42 * Math.PI) / 180
  const L = Array.from({ length: BLADES }, (_, k) => {
    const a = Math.PI / 2 + (k * 2 * Math.PI) / BLADES
    return {
      ox: CX + RO * Math.cos(a), oy: CY - RO * Math.sin(a),
      ix: CX + RI * Math.cos(a - TW), iy: CY - RI * Math.sin(a - TW),
    }
  })
  const opening = L.map((b, i) => `${i === 0 ? 'M' : 'L'} ${b.ix.toFixed(2)} ${b.iy.toFixed(2)}`).join(' ') + ' Z'
  const lines = L.map(b =>
    `<line x1="${b.ix.toFixed(2)}" y1="${b.iy.toFixed(2)}" x2="${b.ox.toFixed(2)}" y2="${b.oy.toFixed(2)}" stroke="#0078D4" stroke-width="1.6" stroke-linecap="round"/>`
  ).join('')
  return `<svg xmlns="http://www.w3.org/2000/svg" width="${px}" height="${px}" viewBox="0 0 24 24" fill="none">` +
    `<circle cx="12" cy="12" r="10.6" stroke="#0078D4" stroke-width="1.5" fill="none"/>` +
    `<path d="${opening}" fill="#0078D4" fill-opacity="0.15"/>${lines}</svg>`
}

export default function Icon() {
  const uri = 'data:image/svg+xml;utf8,' + encodeURIComponent(apertureSvg(32))
  return new ImageResponse(
    (
      <div style={{
        width: '100%', height: '100%', display: 'flex',
        alignItems: 'center', justifyContent: 'center',
        background: 'transparent',
      }}>
        {/* eslint-disable-next-line @next/next/no-img-element */}
        <img width={32} height={32} src={uri} alt="Lens" />
      </div>
    ),
    { ...size }
  )
}
