/// Dart fixture: library root.
///
/// 验收锚点符号：dart_unique_marker_fn / VaultApp / GoldExt / Spinnable /
/// Color / VaultId / kVaultDepth；dart_helper_fn 在本文件被引用 3 次，
/// RepoMap 权重应显著高于其它符号。

import 'utils.dart';

/// 配置枚举。
enum Color { red, green, gold }

/// 可复用自旋行为：声明式 mixin（与 Rust trait 同族）。
mixin Spinnable {
  int spin(int n) => n * 7 + 1;
}

/// 演示应用类：run 内引用 utils.dart 的 dart_helper_fn。
class VaultApp with Spinnable {
  final String label;

  VaultApp(this.label);

  int run(int depth) => dart_helper_fn(depth);
}

/// 静态扩展：为 VaultApp 追加锻造方法。
extension GoldExt on VaultApp {
  String forge() => '$label+gold';
}

/// 验收标记函数：内部三次引用 utils.dart 的 dart_helper_fn。
void dart_unique_marker_fn(VaultApp app) {
  final a = dart_helper_fn(app.run(1));
  final b = dart_helper_fn(app.run(2));
  final c = dart_helper_fn(app.run(3));
  print('$a-$b-$c');
}

/// 类型别名。
typedef VaultId = int;

/// 顶层常量。
const int kVaultDepth = 7;
