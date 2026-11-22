/**
 * Coral Container Project — Thai (th) dictionary, part 2
 * * * Platform integration features introduced in v2.6.0.
 * * * Loaded before i18n.js, which merges this into window.CORAL_I18N.
 */
window.CORAL_I18N_TH_FEATURES = {
  "nav.features": "ฟีเจอร์เชิงแพลตฟอร์ม",
  "feat2.title": "ฟีเจอร์เชิงแพลตฟอร์มระหว่างระบบ (v2.6.0)",
  "feat2.lead":
    "ห้าความสามารถใหม่ที่เพิ่มเข้ามาใน v2.6.0 เขียนด้วย Rust ล้วน ไม่ต้องติดตั้ง daemon ภายนอกเพิ่ม และมี E2E test ครอบคลุม",
  "feat2.cache.title": "Content-Addressable Layer Cache",
  "feat2.cache.desc":
    "coral pull ไม่ดาวน์โหลด layer ที่เคยมีอยู่แล้วอีก ทุก blob ถูกตรวจสอบ SHA-256 ตอนดาวน์โหลดและตอนอ่านจากแคช (ไฟล์ที่เสียจะถูกลบอัตโนมัติ) ย้ายเข้าแคชแบบ atomic ผ่าน rename และดาวน์โหลดขนานสูงสุด 4 ชั้นก่อนประกอบกลับตามลำดับ manifest",
  "feat2.dns.title": "Embedded DNS Service Discovery",
  "feat2.dns.desc":
    "daemon รันเซิร์ฟเวอร์ DNS ตาม RFC 1035 บน UDP ไว้ภายในตัวเอง ลงทะเบียนชื่อคอนเทนเนอร์ ชื่อ service ของ Compose และ --network-alias ให้ resolve ได้ทั้งชื่อธรรมดาและ *.coral.local พร้อมเขียน /etc/hosts และอัปเดตคอนเทนเนอร์ที่รันอยู่แบบ in-place ให้ resolve ได้ทั้งสองทาง",
  "feat2.buildkit.title": "BuildKit Dockerfile Extensions",
  "feat2.buildkit.desc":
    "coral build รองรับ heredoc (RUN <<EOF), RUN --mount=type=cache สำหรับแคชฝั่ง host, type=secret ที่ mount แบบ read-only จาก build context และ type=tmpfs พร้อมคำสั่ง metadata ของ image ได้แก่ EXPOSE, VOLUME, SHELL, HEALTHCHECK และ ONBUILD รวมถึง Multi-stage COPY --from=<stage|index|image>",
  "feat2.dockersock.title": "docker.sock — Docker Engine API",
  "feat2.dockersock.desc":
    "coral-daemon เปิด Docker Engine REST API (/v1.24 ถึง /v1.47) บน Unix socket (ค่าเริ่มต้น <state_dir>/docker.sock โหมด 0660 — เมื่อรันผ่าน systemd unit ของแพ็กเกจจะอยู่ที่ ~/.local/state/coral/daemon/docker.sock) และบน TCP ทำให้ Docker CLI, Testcontainers, Traefik และ Portainer สั่งงาน Coral ได้ผ่าน DOCKER_HOST=unix:// โดยไม่ต้องแก้โค้ดใด ๆ",
  "feat2.gpu.title": "GPU & Device Passthrough",
  "feat2.gpu.desc":
    "ส่งผ่านอุปกรณ์เข้าคอนเทนเนอร์ด้วย --device แบบ Docker-compatible และค้นพบ GPU อัตโนมัติทั้ง NVIDIA, AMD ROCm (/dev/kfd) และ DRM (/dev/dri) พร้อมตัวเลือก --network-alias และ --log-driver (raw, json-file, syslog, journald)",
  "feat2.usageTitle": "ใช้งานอย่างไร",
  "feat2.usageLead": "สรุปคำสั่งสำคัญของแต่ละความสามารถ",
  "feat2.caption": "คำสั่งตัวอย่างสำหรับฟีเจอร์เชิงแพลตฟอร์มใน v2.6.0",
  "feat2.colCapability": "ความสามารถ",
  "feat2.colUsage": "วิธีใช้",
  "feat2.rowCache": "แคช layer ตาม digest",
  "feat2.rowDns": "ค้นหาชื่อบริการในเครือข่าย",
  "feat2.rowBuildkit": "BuildKit heredoc และ mount",
  "feat2.rowDocker": "Docker Engine API",
  "feat2.rowGpu": "GPU และอุปกรณ์",
  "feat2.rowAlias": "Alias และ log driver",
  "feat2.composeLead":
    "คีย์ gpus, devices, log_driver และ aliases ใช้งานได้ในไฟล์ Compose ของ Coral ทั้ง coral-compose.yml และ docker-compose.yml",
};
