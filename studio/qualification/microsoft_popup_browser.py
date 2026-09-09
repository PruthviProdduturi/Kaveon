"""Exercise the real MSAL callback bridge without credentials or an OAuth token.

Run against Studio with public Entra configuration enabled:
python studio/qualification/microsoft_popup_browser.py http://localhost:13003
"""
import base64
import json
import sys
from urllib.parse import urlencode
from playwright.sync_api import sync_playwright

base = sys.argv[1].rstrip("/")
with sync_playwright() as p:
    browser = p.chromium.launch(channel="msedge", headless=True)
    context = browser.new_context()
    page = context.new_page()
    page.goto(base + "/login")
    button = page.get_by_role("button", name="Continue with Microsoft")
    button.wait_for(timeout=60000)
    page.wait_for_function("!document.querySelector('button[aria-busy]')?.disabled", timeout=60000)
    with page.expect_popup(timeout=30000) as popup_event:
        button.click()
    popup = popup_event.value
    popup.close()

    # Listen exactly as MSAL does, then send a synthetic error response through
    # the actual callback page. No sign-in is bypassed and no code is exchanged.
    channel = "popup-browser-regression"
    page.evaluate("name => { window.bridgeMessages = []; window.bridgeChannel = new BroadcastChannel(name); window.bridgeChannel.onmessage = e => window.bridgeMessages.push(e.data); }", channel)
    state = base64.b64encode(json.dumps({"id": channel, "meta": {"interactionType": "popup"}}).encode()).decode()
    callback = context.new_page()
    callback.goto(base + "/auth/microsoft#" + urlencode({"error": "access_denied", "state": state}))
    page.wait_for_function("window.bridgeMessages.length === 1", timeout=60000)
    message = page.evaluate("window.bridgeMessages[0]")
    assert message["v"] == 1 and "access_denied" in message["payload"]
    assert not callback.is_closed() and "#" not in callback.url or callback.is_closed()
    print("PASS: Microsoft popup opens; public callback relays the response and clears the URL.")
    browser.close()
