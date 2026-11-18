#!/bin/bash
# สคริปต์สำหรับ Push โค้ดขึ้น GitHub
cd "$(dirname "$0")"

# เพิ่ม Remote (หากยังไม่มี)
git remote add origin https://github.com/seashyne/Coral-Container-Project.git 2>/dev/null || echo "Remote origin already exists."

# Push โค้ด
echo "Pushing code to GitHub..."
git push -u origin master
