import { ImageResponse } from 'next/og'

export const runtime = 'edge'
export const size = { width: 32, height: 32 }
export const contentType = 'image/png'

// Guardian O — open blue arc, gap at bottom. Matches kaveon-icon.svg.
function guardianOSvg(px: number): string {
  return `<svg xmlns="http://www.w3.org/2000/svg" width="${px}" height="${px}" viewBox="80 80 350 320" fill="none">` +
    `<path d="M 318 363.39 A 124 124 0 1 0 194 363.39" fill="none" stroke="#4A9EE8" stroke-width="52" stroke-linecap="butt"/>` +
    `</svg>`
}

export default function Icon() {
  const uri = 'data:image/svg+xml;utf8,' + encodeURIComponent(guardianOSvg(32))
  return new ImageResponse(
    (
      <div style={{
        width: '100%', height: '100%', display: 'flex',
        alignItems: 'center', justifyContent: 'center',
        background: 'transparent',
      }}>
        {/* eslint-disable-next-line @next/next/no-img-element */}
        <img width={32} height={32} src={uri} alt="Kaveon" />
      </div>
    ),
    { ...size }
  )
}
