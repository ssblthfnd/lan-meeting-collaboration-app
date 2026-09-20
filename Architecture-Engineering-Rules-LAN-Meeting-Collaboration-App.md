# Architecture & Engineering Rules

## LAN Meeting Collaboration App

**Version:** 0.5

---

## Document History

| Version | Change |
| --- | --- |
| 0.3 | Architecture baseline. |
| 0.4 | Added §14.1 participant identity binding; note cardinality constraint (§13, §18); PDF export deferral (§19); §26 time, timezone & ordering rules. Decisions recorded in `docs/adr/`. |
| 0.5 | Canonical note format = GFM-subset Markdown (§13.1); `note_versions.created_by_type` added; `remote_submissions` table added (§12, §13); participant claim status is derived from `participant_sessions`, not stored (§14.1). |

---

# 1. Architecture Principle

Aplikasi menggunakan prinsip:

> **Local-first, LAN realtime, remote offline submission.**

Host adalah authoritative node.

```text
┌──────────────────────────────────────┐
│              HOST DEVICE             │
│                                      │
│  Tauri                              │
│   ├── React/TypeScript UI            │
│   ├── Rust Application                │
│   ├── Local HTTP Server               │
│   ├── WebSocket Server                │
│   └── SQLite                          │
│                                      │
└──────────────┬───────────────────────┘
               │
        Local Network
               │
        LAN Participants
```

Remote:

```text
Remote Participant
       │
       │ file transfer
       ▼
Self-contained HTML
       │
       ▼
Browser
       │
       ▼
Submission JSON
       │
       │ file transfer
       ▼
Host
       │
       ▼
Import → Validate → SQLite
```

Tidak ada remote server dalam architecture baseline.

---

# 2. Technology Stack

## Desktop

Tauri.

## Frontend

React + TypeScript.

## Backend

Rust.

## Database

SQLite.

## Local HTTP

Rust HTTP framework/server.

## Realtime

WebSocket.

## Remote Form

HTML + CSS + JavaScript yang dibundel menjadi satu file.

---

# 3. Separation of Responsibilities

## React

Responsible for:

* presentation;
* user interaction;
* state display;
* Host dashboard;
* participant LAN UI.

React tidak bertanggung jawab terhadap:

* authorization;
* database integrity;
* meeting lock enforcement;
* audit integrity.

## Rust

Responsible for:

* HTTP;
* WebSocket;
* authentication/session;
* authorization;
* business rules;
* database access;
* import/export;
* remote submission validation;
* audit logging.

## SQLite

Source of truth.

Tidak boleh ada state penting yang hanya hidup di frontend.

---

# 4. LAN Networking

Host membuka local HTTP server, misalnya:

```text
0.0.0.0:8765
```

Participant LAN mengakses:

```text
http://HOST_IP:8765/join/{token}
```

Server hanya dimaksudkan untuk jaringan lokal.

Application tidak boleh mengasumsikan bahwa:

```text
127.0.0.1
```

dapat diakses participant lain.

---

# 5. No Public Hosting Requirement

Baseline architecture tidak membutuhkan:

* VPS;
* cloud database;
* web hosting;
* public DNS;
* public HTTPS endpoint;
* reverse proxy;
* remote gateway;
* relay server.

LAN meeting harus tetap berfungsi tanpa internet.

Remote submission juga harus dapat dilakukan tanpa internet pada participant side.

---

# 6. Remote Submission Architecture

Remote Form harus bersifat **self-contained**.

Tidak boleh bergantung pada:

```text
<script src="https://...">
<link href="https://...">
fetch("https://...")
```

untuk fungsi inti.

Seluruh resource penting harus tersedia di dalam file HTML.

Tujuan:

> File dapat dibuka dengan browser dalam keadaan offline.

---

# 7. Remote Form Contract

Setiap generated form memiliki immutable metadata:

```text
schema_version
meeting_id
participant_id
participant_name
meeting_title
meeting_date
generated_at
```

Metadata identitas tidak boleh diedit participant.

Form hanya menyediakan field yang memang boleh diisi participant.

---

# 8. Remote Submission Contract

Submission menggunakan structured schema.

Minimal:

```text
schema_version
meeting_id
participant_id
submitted_at
note
links
```

Schema harus memiliki version.

Contoh:

```text
v1
v2
v3
```

Parser harus mengetahui schema yang didukung.

---

# 9. Submission Integrity

Submission harus dapat diperiksa sebelum import.

Minimal validation:

```text
schema valid?
meeting exists?
meeting ID matches?
participant exists?
participant belongs to meeting?
meeting OPEN?
submission duplicate?
content valid?
link count <= 5?
```

Jika salah satu gagal, submission tidak boleh langsung dipersist.

---

# 10. Remote Submission Security

Remote HTML file dianggap **untrusted input**.

Walaupun file dibuat oleh aplikasi sendiri, data yang dikembalikan participant harus diperlakukan sebagai input eksternal.

Jangan:

* mempercayai nama file;
* mempercayai participant name dari filename;
* langsung INSERT tanpa validation;
* menganggap JSON valid berarti data valid.

---

# 11. Import Transaction

Import harus menggunakan transaction.

Conceptually:

```text
BEGIN TRANSACTION

validate submission
create/update note
create links
create version
create audit log

COMMIT
```

Jika salah satu tahap gagal:

```text
ROLLBACK
```

Tidak boleh terjadi partial import.

---

# 12. Duplicate Submission

Sistem harus mendeteksi submission yang sama.

Minimal identifier:

```text
meeting_id
participant_id
submission/version identifier
```

UI harus memberi tahu Host apabila submission kemungkinan merupakan duplicate.

Host dapat menentukan apakah submission baru:

* ditolak;
* menggantikan versi sebelumnya;
* dibuat sebagai versi baru.

Deteksi menggunakan tabel `remote_submissions` (§13):

* `submission_id` dibuat saat form digenerate dan dibawa kembali oleh submission;
* `content_hash` dihitung dari payload yang dinormalisasi;
* `submission_id` + `content_hash` sama → file yang sama persis, import bersifat idempoten;
* `submission_id` sama tetapi `content_hash` berbeda → koreksi dari participant yang sama;
* keduanya berbeda → submission baru.

Filename tidak pernah digunakan untuk deteksi (§21).

---

# 13. Database Schema

## meetings

```text
id
title
topic
date
start_time
end_time
timezone            -- IANA identifier, e.g. Asia/Makassar (required)
location
description
status
join_token_hash
created_at
updated_at
locked_at
```

`date`, `start_time`, dan `end_time` dimaknai dalam `timezone`. Timestamp sistem (`created_at`, `updated_at`, `locked_at`) disimpan dalam UTC. Lihat §26.

## participants

```text
id
meeting_id
name
department
position
meeting_role
created_at
```

## participant_sessions

```text
id
meeting_id
participant_id
session_token_hash
created_at
last_seen_at
revoked_at
```

## notes

```text
id
meeting_id
participant_id
content
created_at
updated_at

UNIQUE(meeting_id, participant_id)
```

Cardinality: **tepat satu note per participant per meeting**. Constraint `UNIQUE(meeting_id, participant_id)` bersifat wajib dan merupakan enforcement utama, bukan sekadar konvensi aplikasi.

## 13.1 Note Content Format

`notes.content` dan `note_versions.content` menyimpan **GFM-subset Markdown sebagai teks**.

* subset diizinkan melalui allowlist eksplisit; raw HTML ditolak;
* renderer dan sanitizer wajib digunakan di setiap titik render, termasuk Host UI;
* skema link dibatasi pada `http`, `https`, `mailto`;
* renderer TypeScript (`packages/editor`) dan renderer Rust (`app-export`) harus menyetujui subset yang sama, dijaga oleh fixture bersama.

Lihat `docs/adr/0007-note-canonical-format.md`.

## note_links

```text
id
note_id
title
description
url
created_at
updated_at
```

## note_versions

```text
id
note_id
version
content
created_at
created_by_type     -- HOST | PARTICIPANT | REMOTE_IMPORT
created_by          -- participant_id, atau NULL untuk HOST
```

`created_by_type` bersifat wajib. `created_by` sendiri tidak cukup untuk membedakan actor sebagaimana disyaratkan §18.

## audit_logs

```text
id
meeting_id
actor_type
actor_id
action
target_type
target_id
metadata
created_at
```

## remote_submissions

```text
id
meeting_id
participant_id
submission_id       -- identitas stabil, dibuat saat form digenerate
content_hash        -- hash konten submission yang dinormalisasi
imported_at         -- UTC
resolution          -- IMPORTED | REPLACED | REJECTED_DUPLICATE | REJECTED_INVALID
note_version        -- versi note yang dihasilkan, NULL bila ditolak
raw_payload         -- payload submission apa adanya, untuk audit/forensik

UNIQUE(meeting_id, participant_id, submission_id, content_hash)
```

Tabel ini bersifat **wajib**, bukan opsional. Duplicate detection (§12) memerlukan catatan persisten atas submission yang pernah diproses agar import bersifat idempoten: file yang sama persis dapat dikenali sebagai duplicate, sedangkan koreksi dari participant yang sama dapat dibedakan.

`raw_payload` menyimpan input eksternal apa adanya dan **tidak pernah** dibaca sebagai sumber otoritas.

Aktivitas import tetap dicatat pada `audit_logs`; `remote_submissions` adalah catatan idempotency, bukan pengganti audit.

---

# 14. Authorization

Authorization harus dilakukan di Rust/backend.

Contoh:

```text
Participant A
→ note A: allowed

Participant A
→ note B: denied

Participant A
→ participants table: denied

Participant A
→ lock meeting: denied
```

UI hiding bukan security mechanism.

## 14.1 Participant Identity Binding (MVP Decision)

MVP menggunakan **first-claim-wins** identity binding.

Rules:

1. Satu participant identity hanya boleh terikat pada **satu session aktif** dalam satu waktu.
2. Klaim atas identity yang sudah terikat session aktif harus **ditolak backend**.
3. Host dapat melihat claim status setiap participant dan dapat **approve / reject / revoke** klaim dari Host UI.
4. Binding dienforce di backend. UI hanya menampilkan state.
5. `participant_id` yang dikirim client **tidak pernah** dipercaya sebagai otoritas.
6. Setelah binding, **session token** adalah kredensial participant.
7. Hanya **hash** session token yang disimpan (`participant_sessions.session_token_hash`).
8. Setiap request participant harus me-resolve `Actor::Participant` dari session token hash, bukan dari payload.
9. Reconnect menggunakan session credential yang ada dan **tidak boleh** mengubah identitas.
10. Revoke mengisi `revoked_at`; session yang revoked tidak boleh lolos resolusi actor.

Klaim dan perubahan status klaim termasuk mutation yang harus diaudit (§17).

### Claim Status Derivation

Claim status **tidak disimpan sebagai kolom tersendiri**. Status diturunkan dari `participant_sessions`:

```text
UNCLAIMED  -- tidak ada session untuk participant tersebut
PENDING    -- ada session yang belum di-approve Host (bila approval diaktifkan)
CLAIMED    -- ada session aktif (revoked_at IS NULL)
REVOKED    -- seluruh session participant memiliki revoked_at
```

Alasan: menyimpan status secara denormalisasi menciptakan dua sumber kebenaran yang dapat menjadi tidak sinkron ketika session dibuat, kedaluwarsa, atau dicabut. `participant_sessions` sudah memuat seluruh fakta yang dibutuhkan.

Query derivasi wajib memiliki ordering eksplisit (§26.4) dan harus dapat dievaluasi dalam satu query untuk Host UI.

Catatan batas keamanan: transport LAN adalah HTTP polos. Model ini mencegah pengambilalihan identitas oleh sesama peserta pada alur normal, tetapi tidak memberikan kerahasiaan jaringan. Join URL/QR tetap merupakan secret operasional.

---

# 15. Meeting Lock Rule

Semua mutation harus memeriksa:

```text
meeting.status != LOCKED
```

Jika locked:

```text
reject mutation
```

Tidak boleh hanya mengandalkan disabled button.

---

# 16. WebSocket Rules

WebSocket digunakan untuk notification/realtime update.

Pattern:

```text
Client
 ↓
HTTP mutation
 ↓
Rust
 ↓
SQLite transaction
 ↓
Success
 ↓
WebSocket broadcast
```

Bukan:

```text
WebSocket event
 ↓
database
```

WebSocket bukan source of truth.

---

# 17. Audit Rules

Audit log harus:

* append-only;
* timestamped;
* memiliki actor;
* memiliki action;
* memiliki target;
* menyimpan metadata relevan.

Audit tidak boleh diedit melalui normal application flow.

---

# 18. Versioning Rules

Perubahan note penting harus dapat ditelusuri.

Contoh:

```text
Note
 ├── v1
 ├── v2
 └── v3
```

Host edit:

```text
actor_type = HOST
```

Participant edit:

```text
actor_type = PARTICIPANT
```

Remote import:

```text
actor_type = REMOTE_IMPORT
```

Nilai tersebut dipersist pada `note_versions.created_by_type` (§13), dan pada `audit_logs.actor_type`.

## 18.1 Note Cardinality & Import Semantics

Karena setiap participant hanya memiliki satu note (§13):

* note tidak pernah diduplikasi untuk participant yang sama;
* remote import menggunakan **version-aware update/replacement**, bukan insert note baru;
* setiap perubahan menghasilkan row baru pada `note_versions`;
* histori versi tidak boleh dihapus atau ditimpa.

---

# 19. Export Rules

Export harus deterministic.

Input yang sama harus menghasilkan struktur output yang konsisten.

Ordering harus eksplisit dan total (§26.4). Insertion order database tidak boleh menjadi dasar ordering.

Export wajib menyatakan meeting timezone secara eksplisit (§26).

## 19.1 Export Scope

Phase 1: Markdown, TXT / AI Context.

PDF export **sengaja ditunda ke Phase 2** menunggu technical spike tersendiri. Selama Phase 1 tidak boleh ada dependency PDF yang ditambahkan. Lihat `docs/adr/0004-defer-pdf-export.md`.

AI Context tidak boleh melakukan:

* summarization;
* inference;
* interpretation;
* hallucination.

AI Context hanya melakukan formatting/structuring.

---

# 20. Remote HTML Rules

Generated HTML harus:

* standalone;
* offline-capable;
* no external CDN dependency;
* no API dependency;
* no local database dependency;
* no credential;
* no Host server connection;
* no automatic upload.

Submission hanya keluar ketika participant menekan:

> Export Submission

---

# 21. File Naming

Filename hanya convenience.

Contoh:

```text
Weekly-Coordination-Budi-Santoso.html
Weekly-Coordination-Budi-Santoso-submission.json
```

Application tidak boleh menggunakan filename sebagai security/identity mechanism.

---

# 22. Error Handling

Error harus actionable.

Contoh:

```text
Invalid Submission

Reason:
This submission belongs to another meeting.

Expected meeting:
Weekly Coordination Meeting

Detected meeting:
Monthly Review Meeting
```

Hindari error generik seperti:

```text
Something went wrong.
```

jika informasi lebih spesifik tersedia.

---

# 23. Data Ownership

Data meeting berada di device Host.

Remote HTML form tidak menjadi database.

Submission file bukan authoritative source.

SQLite Host tetap authoritative source.

---

# 24. Privacy

Baseline system tidak mengirim meeting data ke internet.

Data hanya berpindah melalui:

* LAN;
* file transfer;
* external communication channel yang dipilih user untuk mengirim remote form/submission.

Aplikasi sendiri tidak menyediakan communication service.

---

# 25. Engineering Rule

Setiap fitur baru harus menjawab:

1. Apa source of truth?
2. Siapa yang authorized?
3. Bagaimana data divalidasi?
4. Bagaimana error ditangani?
5. Apakah perubahan diaudit?
6. Bagaimana meeting lock memengaruhi fitur?
7. Apakah fitur membutuhkan internet?
8. Apakah fitur menambah dependency eksternal?
9. Apakah data dapat diekspor?
10. Apakah behavior tersebut dapat diuji?
11. Bagaimana fitur menangani waktu dan timezone?

---

# 26. Time, Timezone & Ordering Rules

## 26.1 UTC Storage

Seluruh timestamp sistem disimpan dalam UTC menggunakan representasi RFC 3339 / ISO 8601:

```text
created_at
updated_at
submitted_at
imported_at
last_seen_at
locked_at
audit_logs.created_at
```

## 26.2 Explicit Meeting Timezone

Meeting menyimpan `timezone` sebagai IANA timezone identifier.

```text
Asia/Makassar
Asia/Jakarta
Asia/Jayapura
```

`date`, `start_time`, `end_time` dimaknai dalam timezone tersebut.

Aplikasi **tidak boleh** mengambil timezone dari OS secara implisit untuk menentukan makna jadwal meeting. Timezone OS hanya boleh digunakan sebagai *default value yang ditawarkan* saat Host membuat meeting, dan nilai tersebut tetap harus tersimpan eksplisit.

## 26.3 Display & Export

* UI menampilkan jadwal dalam timezone meeting.
* Export mencantumkan timezone meeting secara eksplisit.
* Konversi dilakukan pada layer presentasi/export, bukan pada storage.

## 26.4 Deterministic Ordering

Setiap query yang menghasilkan output untuk UI atau export harus memiliki ordering total dan eksplisit, contoh:

```text
ORDER BY participant.name, participant.id
ORDER BY note_versions.version ASC
ORDER BY audit_logs.created_at, audit_logs.id
```

Tidak boleh bergantung pada urutan insert atau rowid implisit.

## 26.5 Testing

Harus tersedia test untuk:

* konversi UTC ↔ meeting timezone;
* perilaku DST (walaupun target awal Indonesia tidak menggunakan DST);
* stabilitas ordering export.
