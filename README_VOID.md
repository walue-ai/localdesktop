# Void Linux Minimal Setup for Local Desktop

Bu dokümanda Local Desktop uygulamasının Arch Linux yerine minimal Void Linux kurulumu ile nasıl kullanılacağı açıklanmaktadır.

## Konfigürasyon

Void Linux kullanmak için `localdesktop.toml` konfigürasyon dosyanızı oluşturun veya düzenleyin:

```toml
[distribution]
name = "void"

[user]
username = "user"

[command]
# İsteğe bağlı: Varsayılan komutları geçersiz kılın
check = "xbps-query gtk+3 && xbps-query gtk4 && xbps-query libadwaita && xbps-query gnome-calculator"
install = "xbps-install -Su && xbps-install -y gtk+3 gtk4 libadwaita libhandy gnome-calculator gnome-disk-utility wayland-devel mesa-dri dbus gtk4-demo"
launch = "XDG_RUNTIME_DIR=/tmp dbus-daemon --session --fork && export WAYLAND_DISPLAY=wayland-0 && export XDG_SESSION_TYPE=wayland"
```

## Minimal Paket Listesi

Void Linux kullanırken varsayılan olarak aşağıdaki minimal paketler kurulur:

### Temel GTK Kütüphaneleri
- `gtk+3` - GTK3 kütüphaneleri
- `gtk4` - GTK4 kütüphaneleri  
- `libadwaita` - Modern GTK uygulamaları için Adwaita kütüphanesi
- `libhandy` - Mobil uyumlu GTK bileşenleri

### GTK Tabanlı Mobil Uygulamalar
- `gnome-calculator` - GNOME hesap makinesi (mobil uyumlu)
- `gnome-disk-utility` - GNOME disk yöneticisi (portfolio benzeri)
- `gtk4-demo` - GTK4 bileşenlerini test etmek için

### Sistem Desteği
- `wayland-devel` - Wayland protokol desteği
- `mesa-dri` - GPU sürücü desteği
- `dbus` - Sistem mesajlaşma servisi

## Paket Yönetimi

Void Linux XBPS paket yöneticisini kullanır:
- `xbps-install -Su` - Paket veritabanını güncelle ve sistemi yükselt
- `xbps-install -y <paket>` - Paket kur
- `xbps-query <paket>` - Paketin kurulu olup olmadığını kontrol et
- `xbps-query -Rs <desen>` - Paket ara

## Arch Linux'tan Farkları

| Özellik | Arch Linux | Void Linux |
|---------|------------|------------|
| Paket Yöneticisi | pacman | xbps |
| Güncelleme Komutu | `pacman -Syu` | `xbps-install -Su` |
| Kurulum Komutu | `pacman -S` | `xbps-install` |
| Sorgulama Komutu | `pacman -Q` | `xbps-query` |
| Kilit Dosyası | `/var/lib/pacman/db.lck` | `/var/db/xbps/.xbps_*` |
| Dosya Sistemi Kökü | `/data/data/app.polarbear/files/arch` | `/data/data/app.polarbear/files/void` |

## Minimal Kurulum Hedefi

Bu kurulum aşağıdaki hedefleri karşılar:
- ✅ Void Linux (glibc) rootfs
- ❌ Init sistemi yok (minimal)
- ❌ Masaüstü ortamı yok (minimal)
- ✅ localdesktop + Wayland display
- ✅ GTK3/GTK4 + libadwaita toolkit
- ✅ GTK tabanlı mobil araçlar için hazır
- ✅ xbps paket sistemi

## Dağıtımlar Arası Geçiş

Arch Linux'tan Void Linux'a geçmek için:

1. Konfigürasyon dosyanızda `distribution.name = "void"` olarak ayarlayın
2. Mevcut dosya sistemini temizleyin (uygulama bir sonraki çalıştırmada Void Linux'u indirecek)
3. Uygulamayı yeniden başlatın

Not: Dağıtım değiştirmek tüm Linux dosya sisteminin yeniden indirilmesini ve kurulmasını gerektirir.

## Sorun Giderme

### Paket Kurulum Sorunları
- Void Linux depolarının erişilebilir olduğunu kontrol edin
- Paket isimlerinin Void Linux için doğru olduğunu doğrulayın
- XBPS'ye özgü hata mesajları için logları kontrol edin

### GTK Uygulama Sorunları
- Wayland'ın düzgün yapılandırıldığından emin olun
- GTK kütüphanelerinin tam kurulduğunu kontrol edin
- Display server'ın doğru çalıştığını doğrulayın

### Özel Paket Konfigürasyonu
Gerektiğinde ek veya farklı paketler dahil etmek için konfigürasyon dosyanızdaki `command.install` ayarını değiştirerek varsayılan paketleri geçersiz kılabilirsiniz.

## GTK Mobil Uygulama Desteği

Bu minimal kurulum özellikle GTK tabanlı mobil uygulamalar için optimize edilmiştir:
- Modern GTK4 ve libadwaita desteği
- Wayland protokolü ile doğrudan entegrasyon
- Minimal sistem kaynağı kullanımı
- Hızlı başlatma süresi

### Dahil Edilen Mobil Uyumlu Uygulamalar

| Uygulama | Açıklama | Wayland Desteği |
|----------|----------|-----------------|
| `gnome-calculator` | GNOME hesap makinesi - mobil dostu UI | ✅ |
| `gnome-disk-utility` | GNOME dosya/disk yöneticisi - portfolio benzeri | ✅ |
| `gtk4-demo` | GTK4 bileşenlerini test etmek için | ✅ |

### Uygulama Başlatma

Uygulamalar Wayland içinde doğrudan başlatılabilir:
```bash
# Hesap makinesi
gnome-calculator

# Disk yöneticisi  
gnome-disks

# GTK4 demo
gtk4-demo
```

### Ek Uygulamalar

İhtiyaç halinde şu uygulamalar da eklenebilir:
- `vmpk` - GTK müzik klavyesi
- `dialer` - GNOME dialer (mobil UI)
- Diğer GTK4/libadwaita tabanlı mobil uygulamalar
