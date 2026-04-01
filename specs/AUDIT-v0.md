# v0 Spec Audit — готовность к имплементации

---

## 1. Surface lock lifecycle не определен

Spec говорит: "va: non-null pointer — base virtual address (after lock)". Но `IOSurfaceGetBaseAddress` валиден только между Lock/Unlock.

Arena сидит поверх Surface и раздает raw pointer через alloc(). Эти указатели пригодны только пока Surface залочен.

Вопрос: кто и когда лочит Surface?

Варианты:
- **A**: Surface лочит один раз при создании, держит lock навсегда до drop
- **B**: Arena лочит/анлочит при каждом alloc (убивает 5ns цель)
- **C**: Caller лочит/анлочит вручную, alloc работает только в locked state

Рекомендация: **вариант A** — lock при создании, unlock при drop. Просто, предсказуемо, 5ns alloc сохраняется. Spec должен явно это сказать.

---

## 2. IOSurface CFDictionary properties не указаны

IOSurfaceCreate принимает CFDictionary. Spec не указывает какие ключи ставить. Без этого не реализовать.

Нужные ключи (из rane/surface.rs):

| Key | Value | Description |
|-----|-------|-------------|
| kIOSurfaceWidth | size | total bytes as width |
| kIOSurfaceHeight | 1 | single row |
| kIOSurfaceBytesPerElement | 1 | raw bytes |
| kIOSurfaceBytesPerRow | size | full size per row |
| kIOSurfaceAllocSize | size | allocation size |
| kIOSurfacePixelFormat | 0 | raw (no pixel format) |

Добавить в spec секцию surface → mechanism.

---

## 3. Arena alloc algorithm не специфицирован

Spec говорит "atomic fetch_add" но не описывает alignment rounding и edge cases.

Конкретный алгоритм нужен:

```
fn alloc(size, align):
    loop:
        current = cursor.load(Relaxed)
        aligned = (current + align - 1) & !(align - 1)
        new_cursor = aligned + size
        if new_cursor > capacity:
            return None
        if cursor.compare_exchange_weak(current, new_cursor, Relaxed, Relaxed):
            return base_ptr + aligned
```

Или проще (с потенциальной потерей пространства при гонке):

```
fn alloc(size, align):
    // worst case: align-1 bytes wasted
    padded = size + align - 1
    offset = cursor.fetch_add(padded, Relaxed)
    aligned = (offset + align - 1) & !(align - 1)
    if aligned + size > capacity:
        return None  // cursor уже продвинут — пространство потеряно
    return base_ptr + aligned
```

Spec должен выбрать один и задокументировать tradeoff:
- compare_exchange: корректный, но retry loop под contention
- fetch_add: проще, быстрее, но может терять пространство при overshoot

---

## 4. Thread safety модель противоречива

- Surface: "Send but NOT Sync"
- Arena: "Concurrent alloc is safe (atomic)"

Если Arena владеет Surface, и Surface не Sync, то Arena тоже не может быть Sync. Но concurrent alloc требует Sync (или хотя бы &Arena из разных потоков).

Решение: если Surface лочится один раз при создании (пункт 1), то внутренние данные иммутабельны (ref, va, size). Мутабельность только через atomic cursor в Arena. Тогда:
- Surface: Send + Sync (VA не меняется после lock, ref неизменен)
- Arena: Send + Sync (cursor atomic, surface immutable)

Spec должен явно определить Send/Sync для каждого типа.

---

## 5. Drop и CFRelease не указан

IOSurfaceRef — это CFTypeRef. На drop нужен CFRelease. Spec не упоминает это.

Порядок drop для Surface:
1. IOSurfaceUnlock (если locked)
2. CFRelease(ref)

---

## 6. Pool sizing не определен

Pool имеет const generics SLOT_SIZE и SLOTS. Arena создается внутри Pool::new().

Какой размер Arena?
- `SLOT_SIZE * SLOTS`? Не учитывает alignment padding.
- `(SLOT_SIZE + align_padding) * SLOTS`? Какой align_padding?

Spec должен определить: Arena capacity = SLOT_SIZE * SLOTS (slots уже aligned к SLOT_SIZE, который должен быть кратен 64 для AMX).

---

## 7. Pool → Arena → Surface lifetime chain

Spec: "Pool holds Arc to Arena — Slots can outlive the Pool reference but not the Arena"

Но Arena владеет Surface. Если Slot держит Arc<Arena>, то Arena жива пока есть хоть один Slot. Surface тоже жива. Это корректно, но:

- Pool::drop не может освободить Surface пока есть outstanding Slots
- Это может привести к утечке если Slots никогда не возвращаются

Альтернатива: Slot заимствует Pool (&'a Pool), lifetime гарантирует что Pool жив. Проще, нет Arc overhead, но Slot привязан к lifetime Pool.

Spec должен выбрать модель.

---

## 8. Отсутствуют вспомогательные операции

Arena:
- Нет `used() -> usize` — сколько занято
- Нет `remaining() -> usize` — сколько осталось
- Нет `contains(ptr) -> bool` — указатель принадлежит арене?

Pool:
- Нет `available() -> usize` — сколько свободных слотов
- Нет `capacity() -> usize` — общее количество слотов

Не обязательно для v0 но полезно для отладки. Решить: добавить или нет.

---

## 9. HwBuffer trait слишком минимален

Три getter-а (as_ptr, surface_id, size) — это не trait, это struct. Зачем trait если один тип?

Варианта два:
- **Убрать trait**, просто методы на Arena/Slot — проще
- **Оставить trait** как extension point для v1 — DmaBuffer extends HwBuffer

Рекомендация: убрать trait в v0, добавить в v1 когда появится второй тип.

---

## 10. Отсутствует: как отдать IOSurface в ANE

Spec упоминает ANE доступ через "private API" но не определяет как Surface интегрируется с rane.

Rane использует IOSurfaceRef напрямую (передает в _ANEIOSurfaceObject). Значит Surface должна экспозить raw IOSurfaceRef.

Добавить операцию:
| Operation | Output | Notes |
|-----------|--------|-------|
| as_iosurface_ref | IOSurfaceRef | raw handle for ANE/GPU integration |

---

## 11. Error model: пропущены кейсы

| Missing error | When |
|---------------|------|
| ZeroSize | Surface::new(0) или Arena::new(0) |
| SizeTooLarge | IOSurface имеет максимум (зависит от RAM, ~75% physical) |
| AlignNotPowerOfTwo | arena.alloc с alignment не степень двойки |

---

## 12. CFDictionary FFI functions не перечислены

Для IOSurfaceCreate нужны CF функции. Spec не упоминает их.

Нужные:
- CFDictionaryCreateMutable
- CFDictionarySetValue
- CFNumberCreate (kCFNumberSInt64Type)
- CFStringCreateWithCString (для ключей, или использовать extern constants)
- CFRelease

Или: использовать IOSurface C string key constants напрямую (kIOSurfaceAllocSize и т.д. — они extern CFStringRef).

---

## Summary

| # | Issue | Severity | Recommendation |
|---|-------|----------|----------------|
| 1 | Lock lifecycle | HIGH | Lock once at creation, unlock at drop |
| 2 | CFDictionary keys | HIGH | Add property table to spec |
| 3 | Alloc algorithm | HIGH | Specify compare_exchange variant |
| 4 | Send/Sync model | HIGH | Surface + Arena both Send + Sync |
| 5 | CFRelease on drop | MEDIUM | Add to drop sequence |
| 6 | Pool sizing | MEDIUM | SLOT_SIZE * SLOTS, require SLOT_SIZE % 64 == 0 |
| 7 | Lifetime model | MEDIUM | Choose: Arc vs borrow |
| 8 | Helper operations | LOW | Add used/remaining/available |
| 9 | HwBuffer trait | LOW | Remove in v0, add in v1 |
| 10 | ANE integration | MEDIUM | Expose as_iosurface_ref |
| 11 | Missing errors | LOW | Add ZeroSize, SizeTooLarge |
| 12 | CF FFI functions | MEDIUM | List in spec |
