// MSAL opens this registered redirect URI in its popup. It must remain public
// and intentionally contains no application data.
export default function MicrosoftPopupCallback() {
  return null;
}
