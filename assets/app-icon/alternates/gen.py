# SPDX-FileCopyrightText: MailCraft contributors
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Writes the app icon in each colour scheme, from one template and a palette per scheme.

    python3 assets/app-icon/alternates/gen.py <out-dir>

writes calmail-<scheme>.svg for every scheme below. "blue" is the app icon itself
(assets/app-icon/source/mailcraft.svg); the others are kept for the future and not used yet.
"""
import sys, os
out = sys.argv[1]
T = '''<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1932 1932">
  <defs>
    <linearGradient id="tile" x1="1" y1="0" x2="0" y2="1">
      <stop offset="0" stop-color="{tl}"/><stop offset="0.45" stop-color="{tm}"/><stop offset="1" stop-color="{td}"/>
    </linearGradient>
    <linearGradient id="back" x1="0" y1="0" x2="0" y2="1">
      <stop offset="0" stop-color="{bA}"/><stop offset="0.24" stop-color="{bA}"/><stop offset="0.245" stop-color="{bB}"/><stop offset="1" stop-color="{bC}"/>
    </linearGradient>
    <linearGradient id="mid" x1="0" y1="0" x2="0.6" y2="1">
      <stop offset="0" stop-color="{mA}"/><stop offset="1" stop-color="{mB}"/>
    </linearGradient>
    <linearGradient id="reg" x1="0" y1="0" x2="0" y2="1">
      <stop offset="0" stop-color="{r1}"/><stop offset="1" stop-color="{r2}"/>
    </linearGradient>
    <linearGradient id="flap" x1="0" y1="0" x2="0" y2="1">
      <stop offset="0" stop-color="{rf}"/><stop offset="1" stop-color="{r1}"/>
    </linearGradient>
    <linearGradient id="peg" x1="0" y1="0" x2="1" y2="0">
      <stop offset="0" stop-color="{ln}"/><stop offset="1" stop-color="{r1}"/>
    </linearGradient>
    <linearGradient id="gloss" x1="0" y1="0" x2="0.3" y2="1">
      <stop offset="0" stop-color="#ffffff" stop-opacity="0.28"/><stop offset="0.5" stop-color="#ffffff" stop-opacity="0"/>
    </linearGradient>
    <radialGradient id="tglow" cx="0.75" cy="0.05" r="0.8">
      <stop offset="0" stop-color="#ffffff" stop-opacity="0.10"/><stop offset="1" stop-color="#ffffff" stop-opacity="0"/>
    </radialGradient>
    <clipPath id="panel"><rect x="675" y="525" width="990" height="820" rx="52"/></clipPath>
    <filter id="sh" x="-30%" y="-30%" width="160%" height="170%"><feDropShadow dx="0" dy="28" stdDeviation="34" flood-color="{sh}" flood-opacity="0.5"/></filter>
  </defs>
  <rect x="105" y="95" width="1725" height="1730" rx="360" fill="url(#tile)"/>
  <rect x="105" y="95" width="1725" height="1730" rx="360" fill="url(#tglow)"/>
  <!-- back sheet -->
  <path d="M291.6 546.0 Q278 452 372.0 438.2 L1406.0 285.8 Q1500 272 1515.4 365.7 L1674.6 1336.3 Q1690 1430 1596.4 1446.2 L543.6 1628.8 Q450 1645 436.4 1551.0 Z" fill="url(#back)" filter="url(#sh)"/>
  <path d="M291.6 546.0 Q278 452 372.0 438.2 L1406.0 285.8 Q1500 272 1515.4 365.7 L1674.6 1336.3 Q1690 1430 1596.4 1446.2 L543.6 1628.8 Q450 1645 436.4 1551.0 Z" fill="none" stroke="#ffffff" stroke-opacity="0.35" stroke-width="5"/>
  <!-- middle sheet and the page under it -->
  <path d="M606.9 453.4 Q625 355 722.6 377.0 L1437.4 538.0 Q1535 560 1556.4 657.7 L1683.6 1237.3 Q1705 1335 1606.7 1353.5 L498.3 1561.5 Q400 1580 418.1 1481.6 Z" fill="{mS}" filter="url(#sh)"/>
  <path d="M596.8 403.3 Q615 305 712.3 328.0 L1427.7 497.0 Q1525 520 1547.0 617.6 L1673.0 1177.4 Q1695 1275 1596.7 1293.5 L488.3 1501.5 Q390 1520 408.2 1421.7 Z" fill="url(#mid)"/>
  <path d="M596.8 403.3 Q615 305 712.3 328.0 L1427.7 497.0 Q1525 520 1547.0 617.6 L1673.0 1177.4 Q1695 1275 1596.7 1293.5 L488.3 1501.5 Q390 1520 408.2 1421.7 Z" fill="url(#gloss)"/>
  <path d="M596.8 403.3 Q615 305 712.3 328.0 L1427.7 497.0 Q1525 520 1547.0 617.6 L1673.0 1177.4 Q1695 1275 1596.7 1293.5 L488.3 1501.5 Q390 1520 408.2 1421.7 Z" fill="none" stroke="#ffffff" stroke-opacity="0.45" stroke-width="5"/>
  <!-- the calendar page with the envelope on it -->
  <g filter="url(#sh)">
    <rect x="675" y="525" width="990" height="820" rx="52" fill="{pk}"/>
    <g clip-path="url(#panel)" stroke="{ln}" stroke-width="9" stroke-linejoin="round">
      <rect x="690" y="540" width="960" height="125" rx="40" fill="url(#reg)"/>
      <rect x="690" y="610" width="960" height="55" fill="url(#reg)" stroke="none"/>
      <path d="M775 735 L1565 735 L1170 1043 Z" fill="url(#flap)"/>
      <path d="M688 752 L1003 997 L688 1243 Z" fill="url(#reg)"/>
      <path d="M1652 752 L1337 997 L1652 1243 Z" fill="url(#reg)"/>
      <path d="M712 1332 L1058 1052 L1170 1137 L1282 1052 L1628 1332 Z" fill="url(#reg)"/>
    </g>
    <rect x="675" y="525" width="990" height="820" rx="52" fill="none" stroke="{ln}" stroke-opacity="0.5" stroke-width="4"/>
    <circle cx="907" cy="572" r="68" fill="{pk}"/>
    <circle cx="1430" cy="572" r="68" fill="{pk}"/>
    <rect x="872" y="446" width="70" height="156" rx="35" fill="url(#peg)" stroke="{pk}" stroke-width="4"/>
    <rect x="1395" y="446" width="70" height="156" rx="35" fill="url(#peg)" stroke="{pk}" stroke-width="4"/>
  </g>
</svg>
'''
P = {
 "blue":     dict(tl="#4fd8ff", tm="#0f68f0", td="#0634c4", bA="#3fa9f6", bB="#b4d4fb", bC="#6d9ff1", mA="#9acffc", mB="#4d97f2", mS="#6f9cf1", pk="#1c69e2", r1="#58d3fb", r2="#3f96f3", rf="#9ee6ff", ln="#c9f0ff", sh="#03237e"),
 "teal":     dict(tl="#7ff3dc", tm="#13a596", td="#065358", bA="#2fc2b0", bB="#b9efe6", bC="#5fc7b8", mA="#a6f0e2", mB="#3fb9a8", mS="#5cbcae", pk="#0b7f78", r1="#5fe3cf", r2="#2fb3a2", rf="#b0f7ea", ln="#d6fbf3", sh="#03393c"),
 "violet":   dict(tl="#bea8ff", tm="#6a4cf0", td="#2a1696", bA="#8f78fa", bB="#dcd3fe", bC="#9d89f6", mA="#cdbfff", mB="#8770f3", mS="#9b87f2", pk="#4b30cf", r1="#a995ff", r2="#7c63f0", rf="#d9cfff", ln="#ece7ff", sh="#1a0d63"),
 "sunset":   dict(tl="#ffc86c", tm="#f2683a", td="#b0261c", bA="#ff9a57", bB="#ffdcc2", bC="#f7a273", mA="#ffd4a6", mB="#f5874f", mS="#f39b6d", pk="#d84a24", r1="#ffad6e", r2="#f27c43", rf="#ffd9a8", ln="#fff0de", sh="#621208"),
 "green":    dict(tl="#bff27a", tm="#39b26a", td="#0f633c", bA="#5fcf7e", bB="#cff2d7", bC="#7ccf92", mA="#c8f3b0", mB="#55c27a", mS="#72c48c", pk="#1f8a4c", r1="#7fe09a", r2="#45b56d", rf="#c4f6b8", ln="#e6fbe4", sh="#06371f"),
 "graphite": dict(tl="#98abbf", tm="#45556a", td="#1a222d", bA="#6c8096", bB="#d4dce5", bC="#8d9db0", mA="#c3cfdb", mB="#7a8ca2", mS="#8a9aad", pk="#2f3c4c", r1="#9fb1c4", r2="#71849b", rf="#cdd8e3", ln="#ebf0f5", sh="#080b10"),
}
for n, p in P.items():
    open(os.path.join(out, f"calmail-{n}.svg"), "w").write(T.format(**p))
