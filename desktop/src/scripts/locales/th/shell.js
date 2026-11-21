// Topbar, sidebar, statusbar, search and shared actions.
window.CORAL_DESKTOP_LOCALES = window.CORAL_DESKTOP_LOCALES || {};
window.CORAL_DESKTOP_LOCALES.th = window.CORAL_DESKTOP_LOCALES.th || {};
Object.assign(window.CORAL_DESKTOP_LOCALES.th, {
  // Chrome & Topbar
  "app.title": "Coral Desktop",
  "app.name": "Coral",
  "topbar.toggleSidebar": "สลับแถบข้าง (Ctrl+B)",
  "topbar.toggleSidebarAria": "สลับแถบข้าง",
  "topbar.toggleTheme": "สลับธีมสว่าง / มืด",
  "topbar.toggleThemeAria": "สลับธีม",
  "topbar.themeMenu": "ธีมและรูปแบบแอป",
  "topbar.themeShort": "ธีม",
  "topbar.clearSearch": "ล้างคำค้นหา",
  "topbar.langTitle": "สลับภาษา (EN / ไทย)",
  "topbar.langAria": "สลับภาษา",
  "topbar.settings": "ตั้งค่า",
  "topbar.searchShortcut": "Ctrl K",
  "topbar.globalSearchAria": "ค้นหาทั้งแอป",

  // Theme Popover
  "themePop.title": "ธีมและรูปลักษณ์",
  "themePop.allSettings": "ตั้งค่าทั้งหมด",
  "themePop.modeAuto": "ตามระบบ",

  // Sidebar Navigation & Engine Card
  "nav.workloads": "เวิร์กโหลด",
  "nav.containers": "คอนเทนเนอร์",
  "nav.images": "อิมเมจ",
  "nav.volumes": "โวลุ่ม",
  "nav.advanced": "ขั้นสูง",
  "nav.nextgen": "แซนด์บ็อกซ์ & VM",
  "nav.pods": "พ็อด",
  "nav.application": "แอปพลิเคชัน",
  "nav.settings": "ตั้งค่า",
  "engine.running": "เอ็นจินทำงานอยู่",
  "engine.stopped": "เอ็นจินหยุดอยู่",
  "engine.connecting": "กำลังเชื่อมต่อ…",
  "engine.clickConnect": "กดเชื่อมต่อเพื่อเริ่มต้น",
  "engine.runningMeta": "กำลังทำงาน {count}",
  "engine.connect": "เชื่อมต่อ",
  "engine.disconnect": "ตัดการเชื่อมต่อ",
  "engine.connectedBadge": "เชื่อมต่อแล้ว ({mode})",
  "engine.disconnectedBadge": "ยังไม่เชื่อมต่อ",
  "engine.connectPromptFirst": "กรุณาเชื่อมต่อ Coral Engine ก่อน",

  // Statusbar
  "status.defaultLeft": "Coral Desktop",
  "status.criLeft": "Coral v{version}",
  "status.connectedMode": "เชื่อมต่อแล้ว · {mode}",
  "status.pulling": "กำลังดาวน์โหลด {names}…",
  "status.summary": "ทำงานอยู่ {running} · {images} อิมเมจ",

  // Search Placeholders
  "search.containers": "ค้นหาคอนเทนเนอร์, สแต็ก, อิมเมจ…",
  "search.images": "ค้นหาอิมเมจในเครื่องและแค็ตตาล็อก…",
  "search.volumes": "ค้นหาโวลุ่มและพาธ…",
  "search.nextgen": "กรองพรีเซ็ตและเครื่องมือ…",
  "search.pods": "ค้นหา CRI Pod Sandboxes…",
  "search.default": "ค้นหา…",

  // Common Actions & States
  "common.cancel": "ยกเลิก",
  "common.close": "ปิดหน้าต่าง",
  "common.refresh": "รีเฟรช",
  "common.start": "เริ่ม",
  "common.stop": "หยุด",
  "common.remove": "ลบ",
  "common.run": "รัน",
  "common.pull": "ดึงอิมเมจ",
  "common.update": "อัปเดต",
  "common.create": "สร้าง",
  "common.recreate": "สร้างใหม่",
  "common.pullAndRun": "ดึงและรัน",
  "common.installed": "ติดตั้งแล้ว",
  "common.available": "พร้อมให้ดึง",
  "common.pulling": "กำลังดึง…",
  "common.unused": "ไม่ได้ใช้งาน",
  "common.ready": "พร้อมใช้งาน",
  "common.none": "ไม่มี",
  "common.selected": "รายการที่เลือก",
  "common.clearSelection": "ล้างการเลือก",

  "state.running": "กำลังทำงาน",
  "state.exited": "หยุดแล้ว",
  "state.created": "สร้างแล้ว",
  "state.unknown": "ไม่ทราบสถานะ",
  "state.ready": "พร้อม",
  "state.notready": "ไม่พร้อม",
  "health.healthy": "ปกติ",
  "health.unhealthy": "ผิดปกติ",
  "health.starting": "กำลังเริ่ม",

});
