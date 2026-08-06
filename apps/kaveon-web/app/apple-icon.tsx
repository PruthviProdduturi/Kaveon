import { ImageResponse } from 'next/og'

export const runtime = 'edge'
export const size = { width: 180, height: 180 }
export const contentType = 'image/png'

// Guardian O — open blue arc, gap at bottom.
function guardianOSvg(px: number): string {
  return `<svg xmlns="http://www.w3.org/2000/svg" width="${px}" height="${px}" viewBox="0 0 512 512" fill="none">` +
    `<path d="M 343.68 407.88 A 124 124 0 1 0 168.32 407.88" fill="none" stroke="#4A9EE8" stroke-width="52" stroke-linecap="butt"/>` +
    `</svg>`
}

export default function AppleIcon() {
  const uri = 'data:image/svg+xml;utf8,' + encodeURIComponent(guardianOSvg(120))
  return new ImageResponse(
    (
      <div style={{
        width: '100%', height: '100%', display: 'flex',
        alignItems: 'center', justifyContent: 'center',
        background: 'white',
      }}>
        {/* eslint-disable-next-line @next/next/no-img-element */}
        <img width={120} height={120} src={uri} alt="Kaveon" />
      </div>
    ),
    { ...size }
  )
}
