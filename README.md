# CleanSpace — Akıllı depolama temizleyici

Bilgisayardaki gereksiz dosyaları, kopyaları, eski indirmeleri ve büyük dosyaları bulur.
Silmeden önce ne kadar alan kazanılacağını gösterir. macOS, Linux ve Windows.

| Binary | Ne için |
|---|---|
| `cleanspace` | Masaüstü uygulaması (egui) |
| `cleanspace-tui` | Terminal arayüzü — sunucular / cloud makineler için |
| `cleanspace-server` | Anonim kullanım istatistikleri API'si + admin paneli |

## Kategoriler

- **Kopyalar**: aynı boyut + aynı SHA-256 içerik. Her grubun ilk kopyası seçilmez.
- **Gereksiz**: `.tmp .temp .bak .old .log .crdownload .part .dmp`, `*~`, `.DS_Store`, `Thumbs.db`, `desktop.ini` (varsayılan seçili).
- **Eski indirmeler**: İndirilenler klasöründe 90 günden eski dosyalar.
- **Büyük dosyalar**: 500 MB ve üzeri.

Her dosya tek bir kategoride listelenir, yani boyutlar iki kez sayılmaz. Sembolik linkler takip edilmez.
Masaüstü uygulaması dosyaları çöp kutusuna taşır. TUI'de `d` ile çöpe taşınır, `D` ile kalıcı olarak silinir
(sunucularda çöp kutusu yer açmaz).

## Derleme

```sh
cargo build --release                       # hepsi
cargo build --release --no-default-features --features tui --bin cleanspace-tui   # sadece TUI (sunucu, GUI bağımlılığı yok)
cargo build --release -p cleanspace-server
```

## Masaüstü

```sh
cleanspace [KLASÖR]   # klasör verilirse açılır açılmaz taranır
```
Kenar çubuğunda kategoriler, üstte klasör seçimi (Gözat… / hızlı klasörler) ve özet kartları bulunur.
Satıra tıklamak seçer, 📂 ise dosyayı klasöründe gösterir. Açık/koyu tema sistemi izler.

## TUI

```sh
cleanspace-tui [KLASÖR]   # varsayılan: ev dizini
```
`↑↓` gezin · `boşluk` seç · `a/n` tümü/hiçbiri · `←→` kategori · `d` çöpe taşı · `D` kalıcı sil · `r` yeniden tara · `t` istatistik aç/kapa · `q` çık

## Kullanım istatistikleri ve admin paneli

İstatistik gönderimi **opt-in**'dir: ilk açılışta kullanıcıya sorulur, istendiğinde kapatılabilir.
Sadece sayılar gider (dosya sayısı, bayt toplamları, OS, sürüm, rastgele kurulum ID). **Dosya adı veya yolu asla gönderilmez.**
Uygulama `CLEANSPACE_TELEMETRY_URL` ile derlenmezse hiçbir veri gönderilmez ve izin de sorulmaz.

```sh
CLEANSPACE_TELEMETRY_URL=https://stats.example.com/api/events cargo build --release

CLEANSPACE_ADMIN_TOKEN='uzun-gizli-bir-token' \
CLEANSPACE_ADDR=0.0.0.0:8080 CLEANSPACE_DB=/var/lib/cleanspace.db \
cleanspace-server
```

Admin paneli `/admin` adresinde (HTTP Basic auth, kullanıcı adı `admin`, şifre token). Panelde şunlar var:
toplam/aktif kullanıcı, tarama/temizleme sayıları, bulunan/kazanılan alan, OS/arayüz/sürüm kırılımı,
kategori dağılımı, son 30 gün ve kullanıcı listesi. Her kurulum için ayrıntılı olay geçmişi `/admin/user/<id>` adresinde.

> Sunucu düz HTTP konuşur, internete açarken önüne TLS yapan bir reverse proxy (Caddy/nginx) koyun.
