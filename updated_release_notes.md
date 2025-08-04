## Android PRoot Compatibility Fix - v1.2.11

### 🔧 Fundamental Android PRoot Compatibility Fix
Bu release, Android cihazlarda PRoot'un %40'ta takılmasına neden olan temel uyumluluk sorununu çözer. Root privilege emulation'ı kaldırarak Android app sandbox kısıtlamaları içinde çalışmasını sağlar.

### ✅ Çözülen Sorunlar
- **PRoot Binding Sanitization**: `/proc/self/fd/1` ve `/proc/self/fd/2` binding hatalarını çözüldü
- **Android Seccomp Kısıtlamaları**: `PROOT_NO_SECCOMP=1` environment variable eklendi
- **Missing Bindings**: `PROOT_IGNORE_MISSING_BINDINGS=1` ile Android binding kısıtlamaları aşıldı
- **Verbose Debugging**: `PROOT_VERBOSE=9` ile detaylı hata ayıklama bilgisi eklendi
- **Sistem Çağrısı Hataları**: `execve`, `chmod`, `chdir` için "Function not implemented" hataları çözüldü
- **F2FS Filesystem Uyumluluğu**: Android cihazlarda f2fs filesystem compatibility sorunları çözüldü
- **PRoot Binding Optimizasyonu**: Android güvenlik kısıtlamaları nedeniyle problematik /proc binding'leri kaldırıldı
- **Root Privilege Removal**: Android app sandbox uyumluluğu için --root-id flag'i kaldırıldı
- **Stage Execution**: "Simulating Linux system data..." aşaması artık başarıyla tamamlanıyor

### 🔧 Teknik Değişiklikler
- **Temel Sorun**: `--root-id` flag'i fake_id0 extension'ını aktifleştiriyordu ve bu Android'in güvenlik modeliyle çelişiyordu
- **Çözüm**: Root privilege emulation tamamen kaldırıldı, PRoot artık Android app sandbox içinde çalışıyor
- **Android Namespace Kısıtlamaları**: `--kill-on-exit` ve `--sysvipc` namespace taklit işlemleri kaldırıldı
- **F2FS Uyumluluğu**: `--link2symlink` aktif bırakıldı (F2FS filesystem için gerekli)
- **Alternatif Dizin Kullanımı**: ARCH_FS_ROOT `/data/local/tmp/arch` olarak değiştirildi (F2FS yerine daha uyumlu konum)
- **PRoot-Userland Variant**: libproot-userland.so kullanılarak Android app sandbox uyumluluğu sağlandı
- Android güvenlik kısıtlamaları nedeniyle problematik `/proc/self/fd` binding'leri kaldırıldı
- PRoot seccomp filtering'i devre dışı bırakıldı (Android uyumluluğu için)
- Missing binding warnings suppressed (Android compatibility)
- F2FS filesystem compatibility workaround enabled (Android filesystem issues)
- Comprehensive Android-specific PRoot environment configuration
- Four-layer approach: PROOT_NO_SECCOMP + PROOT_VERBOSE + PROOT_IGNORE_MISSING_BINDINGS + PROOT_F2FS_WORKAROUND
- Removed problematic /proc bindings: .loadavg, .stat, .uptime, .version, .vmstat, .sysctl_* files

### 📱 APK Detayları
- **Boyut**: 51MB
- **Mimari**: ARM64 (aarch64-linux-android)
- **Build Türü**: Debug
- **Native Kütüphaneler**: 
  - liblocaldesktop.so
  - libproot-userland.so, libproot_loader.so
  - libxkbcommon.so

### 🔧 Teknik Özellikler
- Arch Linux ARM64 filesystem desteği
- PRoot chroot sistemi (Android uyumlu)
- Wayland compositor
- Xwayland desteği
- XFCE4 masaüstü ortamı

### 📋 Kurulum
APK dosyasını Android cihazınıza indirip yükleyebilirsiniz. Bu sürüm, önceki sürümde yaşanan PRoot runtime hatalarını çözer ve uygulama artık Android cihazınızda düzgün çalışmalıdır.

---
**Link to Devin run**: https://app.devin.ai/sessions/a782075c682b42c7ab9cd8ae1a602d66
**Requested by**: @walue-dev
**Fixed Issues**: PRoot binding sanitization errors, Android seccomp restrictions, system call failures, root privilege emulation conflicts
