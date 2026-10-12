// This device's long-term identity.
//
// A keypair generated once, on first run, and kept in the platform's secure
// storage. The *public* half is sent to a desktop in the join handshake, and
// it is how the desktop recognises this phone across reconnects — which is
// what makes an operator's "Lead Vocal" label and any revocation stick to the
// device rather than to a connection.
//
// The private half is never sent anywhere and is not used for anything yet.
// It exists now because the alternative — deriving an identity from something
// like an install ID — produces an identifier a reinstall changes, and a
// performer losing their channel label because they reinstalled the app would
// be a poor trade for a few lines saved.

import 'dart:convert';

import 'package:cryptography/cryptography.dart';
import 'package:flutter_secure_storage/flutter_secure_storage.dart';

/// Keys under which the identity is stored.
const _privateKeyEntry = 'stagelink.identity.privateKey';
const _publicKeyEntry = 'stagelink.identity.publicKey';

/// This device's identity, as the desktop sees it.
class DeviceIdentity {
  const DeviceIdentity({required this.publicKeyHex});

  /// Ed25519 public key, lower-case hex. Not a secret.
  final String publicKeyHex;

  /// A short, readable form for a diagnostics screen.
  ///
  /// The desktop shows a key fingerprint too, so a person comparing the two
  /// needs something they can read aloud rather than 64 characters.
  String get fingerprint {
    final groups = <String>[];
    for (var index = 0; index < 16; index += 4) {
      groups.add(publicKeyHex.substring(index, index + 4).toUpperCase());
    }
    return groups.join('-');
  }
}

/// Loads or creates the device identity.
class IdentityStore {
  IdentityStore({FlutterSecureStorage? storage})
      : _storage = storage ?? const FlutterSecureStorage();

  final FlutterSecureStorage _storage;

  /// Returns the existing identity, creating one on first run.
  ///
  /// Generating on first run rather than at install time means a device that
  /// has never joined anything has no key, which is one fewer secret sitting
  /// on a phone that might never use the app.
  Future<DeviceIdentity> loadOrCreate() async {
    final existing = await _storage.read(key: _publicKeyEntry);
    if (existing != null && _looksLikeKey(existing)) {
      return DeviceIdentity(publicKeyHex: existing);
    }

    final algorithm = Ed25519();
    final pair = await algorithm.newKeyPair();
    final publicKey = await pair.extractPublicKey();
    final privateBytes = await pair.extractPrivateKeyBytes();

    final publicHex = _hex(publicKey.bytes);
    await _storage.write(key: _privateKeyEntry, value: base64Encode(privateBytes));
    await _storage.write(key: _publicKeyEntry, value: publicHex);
    return DeviceIdentity(publicKeyHex: publicHex);
  }

  /// Forgets the identity.
  ///
  /// Deliberately not offered in the UI as a casual action: the next join
  /// looks like a brand-new device to every desktop that has ever seen this
  /// one, so a label and a revocation are both lost.
  Future<void> forget() async {
    await _storage.delete(key: _privateKeyEntry);
    await _storage.delete(key: _publicKeyEntry);
  }

  static bool _looksLikeKey(String value) =>
      value.length == 64 &&
      RegExp(r'^[0-9a-f]+$').hasMatch(value);

  static String _hex(List<int> bytes) =>
      bytes.map((byte) => byte.toRadixString(16).padLeft(2, '0')).join();
}
