// monero-rust-miner/modules/mining_engine/src/huge_pages.rs
use anyhow::{anyhow, Result};
use std::fs;
use std::io;
use std::path::Path;
use tracing::{error, info, warn};

#[cfg(target_os = "linux")]
use libc::{MAP_ANONYMOUS, MAP_HUGETLB, MAP_PRIVATE, PROT_READ, PROT_WRITE};
#[cfg(target_os = "linux")]
use std::ptr;

pub fn check_and_advise_hugepages() -> Result<()> {
    #[cfg(not(target_os = "linux"))]
    {
        info!("Проверка Huge Pages не применима для текущей ОС ({}).", std::env::consts::OS);
        return Ok(());
    }

    #[cfg(target_os = "linux")]
    {
        info!("Проверка конфигурации Huge Pages (Linux)...");
        let meminfo_path = Path::new("/proc/meminfo");
        if !meminfo_path.exists() {
            warn!("Файл /proc/meminfo не найден. Невозможно проверить состояние Huge Pages.");
            return Ok(());
        }

        let content = fs::read_to_string(meminfo_path)
            .map_err(|e| anyhow!("Не удалось прочитать /proc/meminfo: {}", e))?;

        let mut huge_pages_total_count: Option<u64> = None;
        let mut huge_pages_free_count: Option<u64> = None;
        let mut huge_page_size_kb: Option<u64> = None;

        for line in content.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 {
                match parts[0] {
                    "HugePages_Total:" => huge_pages_total_count = parts[1].parse().ok(),
                    "HugePages_Free:" => huge_pages_free_count = parts[1].parse().ok(),
                    "Hugepagesize:" => huge_page_size_kb = parts[1].parse().ok(),
                    _ => {}
                }
            }
        }

        match (huge_pages_total_count, huge_pages_free_count, huge_page_size_kb) {
            (Some(total), Some(free), Some(size_kb)) if size_kb > 0 => {
                info!(
                    "Обнаружено Huge Pages: Всего: {}, Свободно: {}, Размер каждой: {} KB.",
                    total, free, size_kb
                );

                const REQUIRED_MEMORY_MB_FOR_DATASET_AND_CACHE: f64 = 2336.0; // ~2080MB (Dataset) + ~256MB (Cache)
                let recommended_pages = (REQUIRED_MEMORY_MB_FOR_DATASET_AND_CACHE * 1024.0 / size_kb as f64).ceil() as u64;
                let final_recommended_pages = recommended_pages.max(1280); // Минимум ~2.5GB

                if total < final_recommended_pages {
                    warn!(
                        "КОНФИГУРАЦИЯ HUGE PAGES: Общее количество Huge Pages ({}) меньше рекомендуемого минимума (~{} страниц по {}KB).",
                        total, final_recommended_pages, size_kb
                    );
                    warn!("Это МОЖЕТ СИЛЬНО снизить производительность RandomX.");
                } else if free < recommended_pages {
                    warn!(
                        "КОНФИГУРАЦИЯ HUGE PAGES: Количество свободных Huge Pages ({}) может быть недостаточным для немедленного выделения RandomX Dataset (~{} страниц).",
                        free, recommended_pages
                    );
                } else {
                    info!("Huge Pages: Конфигурация выглядит достаточной.");
                }
                 warn!("Для оптимальной производительности убедитесь, что Transparent Huge Pages (THP) отключены (`cat /sys/kernel/mm/transparent_hugepage/enabled` должен показывать [never]).");
                 warn!("Рекомендации по настройке Huge Pages (выполнять от root):");
                 warn!("  1. Отключить THP (если включены): echo never | sudo tee /sys/kernel/mm/transparent_hugepage/enabled");
                 warn!("  2. Установить количество Huge Pages (пример для {} страниц): sudo sysctl -w vm.nr_hugepages={}", final_recommended_pages, final_recommended_pages);
                 warn!("  3. Для сохранения после перезагрузки: echo 'vm.nr_hugepages={}' | sudo tee /etc/sysctl.d/99-monerominer-hugepages.conf && sudo sysctl -p", final_recommended_pages);
            }
            _ => {
                warn!("Не удалось полностью распарсить информацию о Huge Pages из /proc/meminfo. Проверьте конфигурацию вручную.");
            }
        }
        Ok(())
    }
}

#[cfg(target_os = "linux")]
pub fn try_enable_huge_pages_for_process() -> Result<(), String> {
    info!("Попытка тестового выделения памяти с флагом MAP_HUGETLB...");
    const TEST_ALLOC_SIZE: usize = 2 * 1024 * 1024; // 2MB

    unsafe {
        let addr = libc::mmap(
            ptr::null_mut(),
            TEST_ALLOC_SIZE,
            PROT_READ | PROT_WRITE,
            MAP_PRIVATE | MAP_ANONYMOUS | MAP_HUGETLB,
            -1, 0,
        );

        if addr == libc::MAP_FAILED {
            let os_err = io::Error::last_os_error();
            let err_msg = format!(
                "Не удалось тестово выделить память с MAP_HUGETLB (errno: {} - {}). \
                Возможные причины: Huge Pages не настроены в ядре (vm.nr_hugepages), \
                недостаточно свободных Huge Pages, или у процесса нет прав (RLIMIT_MEMLOCK). \
                Рекомендуется настроить Huge Pages системно.",
                os_err.raw_os_error().unwrap_or(0), os_err.to_string()
            );
            warn!("{}", err_msg);
            return Err(err_msg);
        }

        if libc::munmap(addr, TEST_ALLOC_SIZE) == -1 {
            let os_err = io::Error::last_os_error();
            error!(
                "Не удалось освободить тестово выделенную Huge Page память (errno: {} - {}). Это может привести к утечке памяти.",
                os_err.raw_os_error().unwrap_or(0), os_err.to_string()
            );
        } else {
            info!("Тестовое выделение и освобождение памяти с MAP_HUGETLB успешно. Huge Pages, вероятно, доступны для процесса.");
        }
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub fn try_enable_huge_pages_for_process() -> Result<(), String> {
    info!("Функция try_enable_huge_pages_for_process не применима для ОС, отличных от Linux.");
    Ok(())
}
