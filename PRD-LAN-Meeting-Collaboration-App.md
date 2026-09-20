# Product Requirements Document (PRD)

## LAN Meeting Collaboration App

**Version:** 0.5
**Status:** Product baseline
**Architecture:** Tauri + React/TypeScript + Rust + SQLite
**Primary Network Model:** LAN realtime
**Remote Participation Model:** Self-contained HTML offline submission

---

## Document History

| Version | Change |
| --- | --- |
| 0.3 | Product baseline. |
| 0.4 | Applied four architecture decisions: LAN identity binding (first-claim-wins, §5.2), one note per participant (§13), PDF export deferred to Phase 2 (§20), explicit timezone handling (§7, §25). See `docs/adr/`. |
| 0.5 | Canonical note format ditetapkan sebagai GFM-subset Markdown (§13.3); duplicate detection remote submission bersifat idempoten (§12.2). |

---

## 1. Product Overview

LAN Meeting Collaboration App adalah aplikasi desktop untuk membantu pelaksanaan meeting secara terstruktur.

Aplikasi dijalankan oleh **Host** pada satu komputer. Host membuat meeting, menentukan peserta, kemudian menyediakan akses kepada peserta melalui:

1. **LAN realtime** — peserta yang berada pada jaringan lokal dapat bergabung melalui browser.
2. **Remote offline submission** — peserta yang tidak dapat mengakses LAN menerima file HTML mandiri, mengisinya melalui browser tanpa instalasi atau hosting, kemudian mengirimkan file hasil kembali kepada Host.

Host menjadi sumber data utama dan seluruh data meeting disimpan secara lokal menggunakan SQLite.

Aplikasi tidak bergantung pada cloud, API AI, atau server publik.

---

# 2. Problem Statement

Meeting sering menghasilkan catatan yang:

* tersebar di berbagai aplikasi;
* tidak memiliki format yang konsisten;
* sulit dikumpulkan dari banyak peserta;
* sulit ditelusuri setelah meeting;
* tidak memiliki histori perubahan yang jelas;
* sulit diubah menjadi bahan ringkasan atau laporan;
* menyulitkan peserta remote yang tidak berada pada jaringan lokal.

Aplikasi ini menyediakan satu workflow terstruktur untuk mengumpulkan, mengelola, mengaudit, mengunci, dan mengekspor catatan meeting.

---

# 3. Goals

## 3.1 Primary Goals

1. Host dapat membuat meeting dengan cepat.
2. Host dapat menentukan maksimal 99 peserta.
3. Peserta LAN dapat bergabung melalui browser tanpa instalasi.
4. Peserta dapat memilih identitas yang telah disediakan Host.
5. Peserta dapat menulis dan mengedit catatan sendiri.
6. Host dapat melihat dan mengedit seluruh catatan.
7. Seluruh perubahan penting tercatat dalam audit log.
8. Meeting dapat dikunci sehingga data tidak lagi dapat diubah.
9. Meeting dapat diekspor ke format yang mudah dibaca manusia maupun AI.
10. Peserta remote dapat berpartisipasi tanpa hosting atau instalasi menggunakan self-contained HTML form.
11. Host dapat mengimpor submission remote secara terkontrol.

---

# 4. Non-Goals

MVP tidak mencakup:

* video conference;
* audio conference;
* screen sharing;
* cloud synchronization;
* public meeting server;
* remote realtime collaboration;
* participant account system;
* OAuth;
* native mobile application;
* speech-to-text;
* AI API;
* automatic AI summarization;
* calendar integration;
* email service;
* external database;
* public internet exposure of the Host's local server.

---

# 5. User Roles

## 5.1 Host

Host adalah operator meeting dan pemilik sesi.

Host dapat:

* membuat meeting;
* mengedit meeting sebelum lock;
* menambahkan/menghapus participant;
* melihat participant status;
* melihat seluruh notes;
* mengedit notes;
* melihat histori;
* melakukan import remote submission;
* melakukan lock;
* melakukan export;
* membuat AI Context.

## 5.2 LAN Participant

Participant yang berada pada jaringan lokal.

Dapat:

* membuka join URL;
* melakukan klaim identitas (identity claim) dari daftar yang disediakan Host;
* membuat note;
* mengedit note sendiri;
* menambahkan maksimal 5 link;
* melihat status submission sendiri.

Tidak dapat:

* melihat note peserta lain;
* mengubah identitas setelah identitas terikat (bound);
* mengklaim identitas yang sudah diklaim session aktif lain;
* mengelola participant;
* mengubah meeting;
* lock meeting;
* mengubah audit log.

### 5.2.1 Identity Claim (MVP Decision)

MVP menggunakan model **first-claim-wins**.

Aturan:

1. Setiap participant identity hanya dapat dimiliki oleh **satu session aktif**.
2. Identity yang sudah diklaim tidak dapat diklaim oleh session lain selama session tersebut masih aktif.
3. Host dapat melihat status klaim setiap participant (unclaimed / pending / claimed / revoked).
4. Host dapat **approve** atau **reject** sebuah claim dari Host UI.
5. Host dapat mencabut (revoke) klaim sehingga identity kembali dapat diklaim.
6. Binding identity dienforce oleh backend, bukan oleh UI.
7. `participant_id` yang dikirim browser **tidak pernah** menjadi sumber otoritas.
8. Setelah binding berhasil, **session token** adalah satu-satunya kredensial participant.
9. SQLite hanya menyimpan **hash** dari session token.
10. Reconnect menggunakan session credential yang sudah ada dan **tidak boleh** mengubah identitas.

Model ini tidak menggantikan kebutuhan keamanan jaringan: join URL/QR tetap merupakan secret operasional yang dipegang Host.

## 5.3 Remote Participant

Participant yang tidak berada pada jaringan LAN Host.

Remote participant menggunakan:

> **Self-Contained HTML Form**

Remote participant:

1. menerima file HTML;
2. membuka file menggunakan browser;
3. mengisi form;
4. melakukan export submission;
5. mengirim file submission kepada Host.

Tidak membutuhkan:

* instalasi;
* akun;
* internet;
* hosting;
* VPN;
* koneksi langsung ke Host.

Remote participant bukan participant realtime.

---

# 6. Meeting Lifecycle

```text
DRAFT
  ↓
OPEN
  ↓
LOCKED
  ↓
EXPORTED
```

`EXPORTED` tidak harus menjadi database status. Export dapat direkam melalui audit log.

## DRAFT

Host sedang membuat meeting.

## OPEN

Peserta dapat bergabung dan mengirim/edit notes.

## LOCKED

Tidak ada perubahan meeting atau note yang diperbolehkan.

## EXPORTED

Meeting telah diekspor. Status dapat tetap `LOCKED`; aktivitas export dicatat dalam audit log.

---

# 7. Meeting Creation

Host harus dapat menentukan:

* Title
* Topic
* Date
* Start time
* End time
* Timezone (IANA identifier, contoh: `Asia/Makassar`)
* Location
* Description
* Participant list

Timezone bersifat **wajib** dan disimpan eksplisit. Aplikasi tidak boleh bergantung pada timezone sistem operasi secara implisit. Lihat §25 Time & Timezone.

Participant memiliki:

* Name
* Department
* Position
* Meeting role

Jumlah participant:

> **Minimum: 1
> Maximum: 99**

Host dapat menambahkan participant secara dinamis.

---

# 8. Meeting Access

## 8.1 LAN Access

Ketika meeting dibuka, aplikasi menghasilkan:

```text
http://HOST_IP:PORT/join/TOKEN
```

Host dapat:

* menampilkan URL;
* copy URL;
* menampilkan QR;
* menampilkan QR dalam fullscreen/presentation mode.

Participant scan QR atau membuka URL melalui browser.

---

# 9. Remote Participation

Remote participation menggunakan **Self-Contained HTML Form**.

Host dapat memilih participant dan melakukan:

> Generate Remote Form

Aplikasi menghasilkan file HTML mandiri.

Contoh:

```text
Weekly-Coordination/
└── Budi-Santoso-Meeting-Form.html
```

File tersebut berisi seluruh kebutuhan form:

* meeting metadata;
* participant metadata;
* form UI;
* validation;
* JavaScript;
* schema version;
* submission exporter.

File tidak membutuhkan koneksi internet.

---

# 10. Remote Participant Workflow

```text
Host
 ↓
Generate Remote Form
 ↓
HTML file
 ↓
Send through WhatsApp / Teams / Email / other channel
 ↓
Participant opens file
 ↓
Fill form
 ↓
Export Submission
 ↓
Submission file
 ↓
Send back to Host
 ↓
Host Import Submission
 ↓
Validation
 ↓
Preview
 ↓
Confirm
 ↓
SQLite
```

Remote participant tidak boleh mengubah:

* meeting ID;
* participant ID;
* participant name;
* meeting metadata;
* schema identity.

Informasi tersebut bersifat read-only.

---

# 11. Remote Submission

Submission menggunakan structured data, bukan free-form chat text.

Contoh konsep:

```json
{
  "schema_version": 1,
  "meeting_id": "...",
  "participant_id": "...",
  "participant_name": "...",
  "submitted_at": "...",
  "note": "...",
  "links": []
}
```

Nama file tidak digunakan sebagai sumber identitas.

Validasi menggunakan data di dalam submission.

Field `note` merepresentasikan **satu note milik participant tersebut** (lihat §13). Submission tidak pernah membawa lebih dari satu note.

---

# 12. Remote Submission Import

Submission tidak langsung dimasukkan ke database.

Pipeline:

```text
File
 ↓
Parse
 ↓
Schema validation
 ↓
Meeting validation
 ↓
Participant validation
 ↓
Integrity validation
 ↓
Preview
 ↓
Host confirmation
 ↓
Persist
 ↓
Audit log
```

Jika invalid, Host mendapat alasan kegagalan.

Contoh:

* wrong meeting;
* unknown participant;
* invalid schema;
* malformed data;
* duplicate submission;
* meeting already locked.

## 12.1 Import Semantics (MVP Decision)

Karena setiap participant hanya memiliki **satu note** (§13), import tidak pernah membuat note baru yang berdiri sendiri apabila note participant sudah ada.

Import bersifat **version-aware update/replacement**:

* jika participant belum memiliki note → note dibuat (version 1);
* jika participant sudah memiliki note → note diperbarui dan **version baru** dicatat pada `note_versions`;
* histori sebelumnya tidak pernah dihapus.

Host tetap mengontrol keputusan duplicate (tolak / gantikan / jadikan versi baru) sebelum persist.

## 12.2 Duplicate Detection (MVP Decision)

Setiap remote form membawa **submission identity** yang stabil. Setiap submission yang pernah diimpor dicatat, sehingga:

* mengimpor file yang sama dua kali terdeteksi sebagai duplicate (idempotent);
* mengimpor versi koreksi dari participant yang sama dapat dibedakan dari file yang sama persis;
* Host mendapat informasi yang cukup untuk memilih tolak / gantikan / versi baru.

Deteksi menggunakan identitas submission dan hash konten, bukan nama file.

---

# 13. Notes

## 13.1 Cardinality (MVP Decision)

> **Tepat satu note per participant per meeting.**

Aturan:

* database constraint: `UNIQUE(meeting_id, participant_id)` pada tabel `notes`;
* LAN participant dapat membuat/mengedit note miliknya sendiri;
* Host dapat membuat/mengedit note milik participant mana pun;
* remote submission merepresentasikan note tunggal participant tersebut;
* histori perubahan dipertahankan melalui `note_versions`;
* MVP **tidak** merancang multiple notes per participant.

## 13.2 Content

Participant dapat membuat note dengan:

* plain text;
* paragraph;
* heading;
* ordered list;
* unordered list;
* table;
* link.

Image tidak disimpan sebagai attachment dalam MVP.

Jika diperlukan, image hanya dapat direpresentasikan melalui URL/link.

## 13.3 Canonical Format (MVP Decision)

Note disimpan sebagai **teks Markdown dengan subset GFM**.

* `notes.content` dan `note_versions.content` berisi Markdown text.
* Subset yang diizinkan bersifat eksplisit (allowlist): paragraph, heading, ordered list, unordered list, table, link, inline emphasis, code.
* HTML mentah di dalam Markdown **tidak** diizinkan.
* Skema link tetap dibatasi pada `http`, `https`, dan `mailto`.
* Editor dan renderer identik di ketiga UI bundle melalui `packages/editor`.
* Konten participant diperlakukan sebagai untrusted input saat dirender, termasuk di Host UI.

Keputusan ini membuat export Markdown dan AI Context bersifat langsung dan deterministik. Lihat `docs/adr/0007-note-canonical-format.md`.

---

# 14. Links

Setiap note dapat memiliki maksimal:

> **5 links**

Setiap link memiliki:

* title;
* description;
* URL.

Backend wajib melakukan validasi batas maksimal.

---

# 15. Note Editing

Participant:

* dapat membuat note;
* dapat mengedit note sendiri selama meeting OPEN.

Host:

* dapat melihat semua note;
* dapat mengedit semua note selama meeting OPEN.

Setelah meeting LOCKED:

> Tidak ada editing.

Frontend tidak boleh menjadi satu-satunya mekanisme enforcement.

---

# 16. Audit Trail

Perubahan penting harus dicatat.

Contoh:

* meeting created;
* meeting updated;
* participant added;
* participant removed;
* participant joined;
* note created;
* note updated;
* note imported;
* note edited by host;
* remote submission imported;
* meeting locked;
* export generated.

Audit log bersifat append-only dari perspektif user.

---

# 17. Version History

Note mendukung versioning.

Setiap perubahan note dapat menghasilkan:

```text
Version 1
Version 2
Version 3
...
```

Host dapat melihat histori perubahan.

---

# 18. Real-Time Collaboration

LAN participant menggunakan WebSocket untuk update realtime.

Contoh:

```text
Participant A submits note
        ↓
SQLite transaction succeeds
        ↓
Server broadcasts event
        ↓
Host dashboard updates
```

WebSocket bukan source of truth.

SQLite tetap menjadi source of truth.

---

# 19. Locking

Host dapat melakukan:

> Lock Meeting

Setelah locked:

* participant tidak dapat mengubah note;
* host tidak dapat mengubah note;
* participant tidak dapat ditambahkan;
* remote submission baru ditolak;
* meeting metadata tidak dapat diubah.

Backend wajib memvalidasi status meeting.

---

# 20. Export

Phase 1 (MVP):

* Markdown
* TXT / AI Context

Phase 2 (deferred):

* PDF — **sengaja ditunda**, menunggu technical spike tersendiri. Lihat `docs/adr/0004-defer-pdf-export.md`.

Future:

* DOCX
* JSON

Export harus mempertahankan:

* meeting metadata;
* meeting timezone (eksplisit);
* participant;
* notes;
* links;
* timestamps;
* relevant audit information.

---

# 21. AI Context

Aplikasi tidak menggunakan AI API.

Aplikasi menghasilkan context/prompt yang dapat diberikan secara manual kepada:

* ChatGPT;
* Claude;
* Gemini;
* model AI lain.

AI Context harus:

* mempertahankan fakta;
* mempertahankan nama;
* mempertahankan angka;
* mempertahankan tanggal;
* mempertahankan URL;
* membedakan discussion;
* membedakan decision;
* membedakan action item;
* mempertahankan ambiguity;
* tidak mengarang informasi.

---

# 22. Security Requirements

1. Join token harus unpredictable.
2. Token tidak disimpan plaintext jika tidak diperlukan.
3. Session token harus divalidasi server-side.
4. Authorization harus dilakukan di backend.
5. Meeting lock harus enforced backend.
6. Participant hanya dapat mengakses resource yang diizinkan.
7. SQLite tidak boleh diekspos ke jaringan.
8. Tidak ada database port yang dibuka ke LAN.
9. Remote HTML form tidak boleh memiliki akses langsung ke SQLite.
10. Submission remote harus divalidasi sebelum import.
11. Audit log tidak boleh dapat diedit melalui UI.
12. Remote form harus membawa schema version.
13. Import harus mendeteksi submission yang tidak sesuai meeting.
14. Application tidak membutuhkan koneksi internet untuk fungsi LAN maupun remote submission.
15. Participant identity binding dienforce backend (first-claim-wins, §5.2.1).
16. `participant_id` dari browser tidak pernah menjadi otoritas; otoritas berasal dari session token yang divalidasi server-side.
17. Session token hanya disimpan sebagai hash.
18. Reconnect tidak boleh dapat digunakan untuk berpindah identitas.

---

# 23. Technical Constraints

* Desktop Host: Tauri
* UI: React + TypeScript
* Backend: Rust
* Database: SQLite
* LAN server: Rust HTTP server
* Realtime: WebSocket
* Remote submission: Self-contained HTML
* QR: generated locally
* AI: manual export only

---

# 24. MVP Definition

MVP dianggap selesai apabila:

```text
Create Meeting
 ↓
Add Participants
 ↓
Generate QR
 ↓
LAN Participant Joins
 ↓
Select Identity
 ↓
Write Note
 ↓
Host Sees Note
 ↓
Host Edits Note
 ↓
Audit Recorded
 ↓
Generate Remote HTML
 ↓
Remote Participant Fills Form
 ↓
Export Submission
 ↓
Host Imports Submission
 ↓
Validate + Preview
 ↓
Confirm Import
 ↓
Lock Meeting
 ↓
Export
```

Seluruh workflow tersebut harus dapat berjalan tanpa cloud hosting.

Catatan: PDF export bukan bagian dari MVP (§20).

---

# 25. Time & Timezone

Waktu ditangani secara **eksplisit**.

## 25.1 Storage

Seluruh timestamp sistem disimpan dalam **UTC**:

* `created_at`
* `updated_at`
* `submitted_at`
* `imported_at`
* `last_seen_at`
* `locked_at`
* audit timestamp

Representasi menggunakan RFC 3339 / ISO 8601.

## 25.2 Meeting Schedule

Jadwal meeting mempertahankan **intended local timezone**:

* meeting menyimpan `timezone` sebagai IANA timezone identifier (contoh `Asia/Makassar`, `Asia/Jakarta`);
* `date`, `start_time`, dan `end_time` dimaknai dalam timezone tersebut;
* aplikasi tidak boleh bergantung pada timezone OS secara implisit.

## 25.3 Display

UI menampilkan tanggal/waktu meeting dalam timezone meeting, bukan timezone perangkat pembaca.

## 25.4 Export

Export wajib menyatakan timezone meeting secara eksplisit sehingga dokumen dapat dibaca tanpa ambiguitas.

## 25.5 Ordering

Ordering harus deterministik dan eksplisit. Ordering tidak boleh bergantung pada urutan insert database.

## 25.6 Testing

Harus ada test yang mencakup konversi timezone dan perilaku DST, walaupun target awal (Indonesia) tidak menggunakan DST.
