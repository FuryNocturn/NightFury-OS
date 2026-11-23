// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Pruebas de `pk`: vectores de RFC 6979 (ECDSA), RFC 5903 (en `ec`) y firmas RSA/ECDSA de OpenSSL.
use super::*;
use crate::crypto::from_hex;

// Generados con OpenSSL (RSA 2048 y 4096, e = 65537; EC P-256 y P-384) sobre el mensaje MSG.
const MSG: &[u8] = b"NightFury 0.12.2";
const N2048: &str = "b7cbc088605c3eed5ea0ca4797b9444caea3668f6cca7ef4e67eae977091c0743980b87e767b5fd84d6b70934b07aa27e5ef7d5746c730ab6a6b2a82768159c608f274c1edb5761662f5c3be77927398cdeccde331ec47272bf624645e2e2ced0a9d2aa13d242ffe846ac22797b855e1ce94604fb552d1022cf3831dab6691d08de6a65082700e493ccded7d922f30f8c215e4a328818d6ecc04178b888b8586eb8576c4d8f478dbb30496f39032e9c1277a20d9ba630dee7313eb0ca3efe9605f97ad5c4f47ddc3ab6fbc8c468f816ce4daccf477edfd8f1a5a32ff4c3d58b8cc2da6ac575e92a121cd45be7a2cd099f6c27b4e16d6690f07c873acb6fc3d33";
const N4096: &str = "a64e23a1fe78dd9531dab5464e3f421234b695f408503201df4f7c8c3c0fe7d41ab2807d2f1dd656d231010a0ebd716f95984201fb9a609d49f68eac2b35067a30736e223f3515e37f5bb29b0e9559df4627d81d6110933d016c4c416ffe9c6d6a93ed2b64e589f633b1b3ef80e14c8175a66a474afbaa13d33bc3b71feba5cf98ae0c7749c6b21fac90b6e28bb29a6185730ab9eaa6f91225d5a29e84e94134ea8b10226d91a72176418729537818f2a02afdbb78ec01a1c92e18f7cd16d47df98f74a2fb32935442d363a0d9ac4dbc16b96772171a4cbe7d3805181f2acaf3b931cf6d956fb655aaaa7f51db209b69c47838a0a90ae97a53943a0adc26e9c2019d4f9b8474bf73b87dde982e51a4a37b7d1f506a9a6b16fd00644e8e8d304def6efeb2b160e1a79630e9b801489645517e17a68ed701498745d4956c0396ef417f1432e5bef4645ed69e12e6229ab03ffb8c16fafc347e47be5a4e72558dc387421a90d1571f838bbebc81b2c827d4cbf847c26e5bab96defd9617cadcfa5606876dbce37f9cbfd388dd0e403ad3a755a5bf4fff61e390ceec71f5e2d39111ae6a605ed595b678414a47e1261be14d444ebeb6cab1b86ef18af1dd6ab3a5669ecdb3f148c0394b2114e65031bcd8178a7e0c12bfee6c7c61d32b0fc3e7e3f24d2f59ede572733c3618c0eeea0251592c85e65b6c14cc3d9d6c9a30c94c60c9";
const P1_256_2048: &str = "6d7464212e2065137cec82893ce4bd1904abf2566569b358d3b74f1dabbaf0905b172129834020eea180d9bf2cbcecade6b946de1db93b2ac46cded25172b921ac5ac4cf597ab35d8bf1c6a57e51bee649088271fb3e9aa9097b915b93fee27575c522046833c8e95b3d370db5c70cab5059ece5bfb1cecb7b7495252721c11f1640295c47808f0621671cffa9c9a47b97b8dc5054c3497983bb1c06112eecbd0adfbc3c3cc47c82d758e6b37deaf3774dd8308ef50038b7d9382c208709b1f311ab41f8115dfb7b342834285d1fac20db1c40a7ebdbe34a27cd52271a221232dfffd21ec08899b6369777f3f0d397e2fb106a2a57644886120ea38c08f7b1db";
const P1_384_2048: &str = "a015f4e91fb52e6c441a49f943d3f2c42db8ee161f2b1129e3aeaeeb3d37dc4f8d670676c0dd17279a4e8be0c802958be80ca17c0d0a3565a14fa28046dbf50d911650e51bac7fa937b3a206fffaef66841855c556a86aa2997c8a52befe6f9baead0039404f667706018af23e7de0da02c35e7eb11a7dcb7eb783c023bcc70f8a994775f4b38c6eb81cc5da592fc4bc5e538812afc4c9e8e9210c36056b4c211d9b2b2544f239b21740f0aaa858c1e3acd733bd17fa2ea7180946162c413e0b7b5b1d4d687b279179a478b2556a3af2652efa042aaa2609985a3f6ba154296e27823efd6115dfb2e3b959f40b68fea8ac7928dfca895447096c4ed694340a64";
const PSS_256_2048: &str = "5b55090599ca4bc5838d1029abc1c57fba52f7c2612faf4f8d4c4d33baf7828bae51a5984549eb69adb471e6fb6978989db799abccf7d6000ff718f73343978be92d6ea1f6bca74567d639f85d201f784ab22fb8b5ff676c2bb6e7103307173c4ca8ac7c783b49c6b1c1c831ab566083c10d8325489ef6cdeb4523c0e010291f33bc10e275089302920ac167ca021b2cdd695006b53f812a53641c22a4d75facaedd9ea092c23372c1d64481f26bd69f14dcfdc6158c36beb003a52107055e657fa0366b33feae960bbef328d095ce253e61d9738215a0642f852d7922b610d73630939be07a7ca675a9fd9bd7180ef5747b923da71a654fc0ffd04c228b856a";
const PSS_512_2048: &str = "45bd843f8240bfce7950714812fb73d83738d38a0a280041aac0fb676ae07bd17c2e3f68c065887268c01761ce7ded5982e0b7f0df131149d89b708609db74cab99b460881253a5634baac378238b5cc8fbddf4bdb0c9b289d94ed00295cb726a736262a6194641432449e4e9a2fa24447e252108de3e1cd20da2a5f2ecef1957ae3d163a8f23f14bdce74e5f9e87c5ad808213babe7c1e38f9c51263bde83da40cb149b361712c5806be5b90ab3db59f6c85f7655078e358bc12bb17c6ff989fdb18b75d482c622c25bbbadccac2e2d40a248834c08332454bbdb7fc2a320152d38b5591e972a69991d41c46c8abfde6ae3e6d40a29b3ad9364c9b7f226dd79";
const P1_512_4096: &str = "45a2aee5c0719c9fd8ea779a8ea9ee81253ed36f1a74b3dc4eaca9b9008c6b766956e95f11aac6ed4ea314b5d9dfddd2d4acebbdaabe27d257b1941e1292b1421a9b5060ec4ea265f2f94dfa62042988dea805f8ea6fd29261235abda6fad4525d10fce20546681afdb0cc732c3a735f8e7ac41c55218773fcfe76c7c1b36d059f71e4f97a9a51ef0b065c3a44f5bc19182510a4bd1c8127b344a8d12d9967e4a9f98beb5de5c9f6a9ea0a0013a4d991b15c20da0e65cd5bce2a86de5cd1347874b42c0218ee5b1ff9713d8d5d6a8e6da6d931285cd30ac76e8e3bd8c6f7b3b7e6871d080d0ef7dd3b5cfa96d0eab4f86e4c3c16fff3d719f94e2f8fb65c4dbab82faba4670327a4dc8c5ecf44bcb6c53d2a0a1652cc3bb7c21c96261cd95402bd418700016159e6c99dc396d7fbc6f9b36ac61d651dd00d555a1dec9fc082a954790522ba24afbc05a45a25bcae5cd36157406a7f34f9f33beea2c399c87283111442fddadc19bb615391a1fe9f6ba3136d7118e0b7584e9bfc48fa729dae70e2341e07ad43994f16dbce97802138df5be201fc0b6c2372cd7bebcfe70314785de60d83cda2d0b9146210caf3ce84f03b5203c746e6a2104669fb3e934d79fcb68ffa70c7c4cb6e99f4e7e61d1f850485829a23586dcd8a4ae8bb94baf067b73214ef5964ab6a60a075c81c77a7c04e9beadccca5f0fd66ab7728406ac4c690";
const PSS_384_4096: &str = "30581a914cb86f34965a9c5c9baddd2fc3fbae367f03264998cc6757fc222a6fbebb35fb42a019861f417fca4648b0bc995ca16e517aab626df6760ae9499db364da8f1d7f3bb319c1f8df507a2606cee6e8a2dacaa0e216c899ad975f9efdfbbcd88ef4b4d912a1e1fa13b4f1c894df088b3b0aa04cd480b11016aef561fe814ed2ae9a626cf379249f9a6a75c6c906f07ef0f798f83bee5e5d7e68ec94cfe3206c0dc24a9992ffaea3cc83bfc1a3a2df26b42bc74ef1dc99eb4d2e58343e21e8d197f1ec099acd033b2a63586f9edc0fe700570bfbb0a68f0eb2a7840b4f7e68f5b3b5322918fb5eab39c770a2a4eafcd4c0d637b62cfcabd2a96b2b048494cb6a7e596f6d948bb58d697b3d00c1d9164b6ae5106a85c82933167c2bba6747bf67e54695ab9ea3cf4d04b1b35368bf6a4daa22079dc4db735556efc5034ea70fde31ddaadeab1ed73f1f0b541ce09cfe00d09d94d48d46c4d4e3cd9c84475401c21961b4a83d68e3325e571ab7584345d41ab488643013835b38d7a29dea1e1b24275d4ca454f8790f766c70c5036527f0ebdf38c311e0005d3458627c1a4b7f6772c778e752ffda028bbbedf8765792b1361c64ece5ee05a7f01a1f22cdee3d0f9a287e39438803d29b67d574be650ad29f362b44a25bccebabfc02187fcf96eb6f9a2abfa0064d1aa62211f003ae97cb6552ff2b935f3edfaa21e3daa1ce";
const P1_256_4096: &str = "5aa6a5dac6468c7fcdcde4ff9956c3605a0e9e9e5b171be306b60ab5bd33e78716871306c4f1aef56dc8d21f788e91a738de76bc1169652342212ac0a1e577dd0f989ad05a593d61d5c2a8bbe2092253618caf1fbd9fa1e4c89f29a2523b57da87697bfc7dc1f3aeff4e1ca7c290ed0c881babe7371bf5e48b62525f35a5cc19cc62e7981d04e05fb06903dc736c61cf2722e4cafffcbe8a51a74da4400b0cae2a3953b33d3b65e6d50a19b254951f17c5d3de26ba08cfa340af3a854ac80a8994ce1f36088ffc9e2e0cb207000b00e4f1befc82483419a3116666d0656b6f2b12886ffb5c22e1dc9915e6938466d1dcfb500606d9927d4e00f89cdcea6343ec2b37d383759f2745902abe557ccb5a871aadaa71821164b11213b7b22d5ee8a4e441c5f43ebd2e1a0f2a218c3c9023c15105c57bf8bf5bebe02725cb7d3803a3fd7323cfda9dd0f740a966c31ec82c11286aebb13723f4f6fac6505ffaa6e939ab0ea9592c3517628ae4dd3c47bd66de8e6b4e0528c994dadbfc0243d1dea2bd3b36d2f4ec0eec9825166ce33b327dfc2cfb48c4582cf80bb9fbcc895565acae6a7a9a749a3cb106c17cae056842174af5d8ee998647ebb164375f93d414cda58a4b85e451da231be799f00183f4f40b793e7ba33ab2cebb40eb22bbc9b7ab541dc50a3146b6955af0972372c03a85cb3aa4d4ffcb66608809af0814da3698a6";
const EC256_SHA384: &str = "304302207e3ba2a285028290f00235b0e1ecc6f7ca06d37caba433a7905d6e5331442e9b021f661abb38ceddf217e5421956ef70255660b574125341436747f66e3ec951bc";
const EC384_SHA256: &str = "306502300b38128cfeed084a2d7c03cd6142a80d61636cfe6d933967d1c5ae7d1d52419bc6ea684499134da82d307ef685cf7660023100ed8f1d1db8d027bcfcd408e8c1fb50b91efe298f86a7f076c8459be128ee2520afe0084a79673bbbc40bb44b1e49c46f";
const EC384_SHA512: &str = "306402306acbe94b238d5a4b98431dc11c62b243d622c75e58d818cb72473d13d67df5eeea012cea6f6f1fd4a2ec6c25e2be18290230585a7235fa27b595c4949caecf50e050e0c155a2ed9e91ff5a30163a61485cd3bc891d4455a0a22e4d75ad75df00c552";
const Q256: &str = "04786ccd9130e76d4314568386d3409badbe2b02bd8d8ea0f7cf203fac6bc5a993852049386a19dc145ca8e7625f6102005796d851b56152532d17b8e69fec05df";
const Q384: &str = "04d2436b371b658e93fbe4834a48cc6ab38df575ff4621c9a2771b51de7e40138f765526843a9b5f5fd9ce897ea3516d59fc3fc0c18c998fdcffbb49adb138faf206390fc46158e1ba9e6fac11e47995e2c44e35707e6c968f0cc25d769edf8f68";

fn hx(s: &str) -> Vec<u8> {
    from_hex(s).unwrap()
}

/// SEQUENCE { INTEGER r, INTEGER s } con la longitud mínima.
fn der(r: &[u8], s: &[u8]) -> Vec<u8> {
    let int = |v: &[u8]| {
        let v = &v[v.iter().take_while(|&&b| b == 0).count()..];
        let mut o = vec![2, (v.len() + (v[0] >> 7) as usize) as u8];
        if v[0] & 0x80 != 0 {
            o.push(0);
        }
        o.extend_from_slice(v);
        o
    };
    let body = [int(r), int(s)].concat();
    let mut o = vec![0x30];
    if body.len() >= 0x80 {
        o.push(0x81);
    }
    o.push(body.len() as u8);
    o.extend(body);
    o
}

fn rsa(n: &str) -> PubKey {
    PubKey::Rsa { n: hx(n), e: vec![1, 0, 1] }
}

#[test]
fn ecdsa_p256_rfc_6979_a_2_5() {
    let q = hx("0460fed4ba255a9d31c961eb74c6356d68c049b8923b61fa6ce669622e60f29fb67903fe1008b8bc99a41ae9e95628bc64f2f1b20c2d7e9f5177a3c294d4462299");
    let key = PubKey::EcP256(q);
    let r = "efd48b2aacb6a8fd1140dd9cd45e81d69d2c877b56aaf991c34d0ea84eaf3716";
    let s = "f7cb1c942d657c41d436c7a1b6e29f65f3e900dbb9aff4064dc4ab2f843acda8";
    let sig = der(&hx(r), &hx(s));
    assert!(verify(&key, SigAlg::EcdsaSha256, b"sample", &sig));
    // Mensaje distinto, firma alterada, otro hash y algoritmo RSA con clave EC: todo falla.
    assert!(!verify(&key, SigAlg::EcdsaSha256, b"sampld", &sig));
    assert!(!verify(&key, SigAlg::EcdsaSha384, b"sample", &sig));
    assert!(!verify(&key, SigAlg::RsaPkcs1Sha256, b"sample", &sig));
    for i in 0..sig.len() {
        let mut m = sig.clone();
        m[i] ^= 0x01;
        assert!(!verify(&key, SigAlg::EcdsaSha256, b"sample", &m), "byte {i}");
    }
    // Segundo vector de la misma sección: SHA-256 de "test".
    let sig = der(
        &hx("f1abb023518351cd71d881567b1ea663ed3efcf6c5132b354f28d3b0b7d38367"),
        &hx("019f4113742a2b14bd25926b49c649155f267e60d3814b4c0cc84250e46f0083"),
    );
    assert!(verify(&key, SigAlg::EcdsaSha256, b"test", &sig));
}

#[test]
fn ecdsa_p384_rfc_6979_a_2_6() {
    let q = hx("04ec3a4e415b4e19a4568618029f427fa5da9a8bc4ae92e02e06aae5286b300c64def8f0ea9055866064a254515480bc138015d9b72d7d57244ea8ef9ac0c621896708a59367f9dfb9f54ca84b3f1c9db1288b231c3ae0d4fe7344fd2533264720");
    let key = PubKey::EcP384(q);
    let sig = der(
        &hx("94edbb92a5ecb8aad4736e56c691916b3f88140666ce9fa73d64c4ea95ad133c81a648152e44acf96e36dd1e80fabe46"),
        &hx("99ef4aeb15f178cea1fe40db2603138f130e740a19624526203b6351d0a3a94fa329c145786e679e7b82c71a38628ac8"),
    );
    assert!(verify(&key, SigAlg::EcdsaSha384, b"sample", &sig));
    assert!(!verify(&key, SigAlg::EcdsaSha384, b"Sample", &sig));
    assert!(!verify(&key, SigAlg::EcdsaSha256, b"sample", &sig));
    assert!(!verify(&PubKey::EcP256(hx(&"04".repeat(65))), SigAlg::EcdsaSha384, b"sample", &sig));
    for i in (0..sig.len()).step_by(3) {
        let mut m = sig.clone();
        m[i] ^= 0x80;
        assert!(!verify(&key, SigAlg::EcdsaSha384, b"sample", &m), "byte {i}");
    }
}

#[test]
fn ecdsa_openssl_hashes_cruzados() {
    let k256 = PubKey::EcP256(hx(Q256));
    let k384 = PubKey::EcP384(hx(Q384));
    assert!(verify(&k256, SigAlg::EcdsaSha384, MSG, &hx(EC256_SHA384)));
    assert!(verify(&k384, SigAlg::EcdsaSha256, MSG, &hx(EC384_SHA256)));
    assert!(verify(&k384, SigAlg::EcdsaSha512, MSG, &hx(EC384_SHA512)));
    assert!(!verify(&k256, SigAlg::EcdsaSha256, MSG, &hx(EC256_SHA384)));
    assert!(!verify(&k384, SigAlg::EcdsaSha384, MSG, &hx(EC384_SHA256)));
    assert!(!verify(&k384, SigAlg::EcdsaSha512, b"otro", &hx(EC384_SHA512)));
}

#[test]
fn ecdsa_der_estricto() {
    let key = PubKey::EcP256(hx("0460fed4ba255a9d31c961eb74c6356d68c049b8923b61fa6ce669622e60f29fb67903fe1008b8bc99a41ae9e95628bc64f2f1b20c2d7e9f5177a3c294d4462299"));
    let sig = der(
        &hx("efd48b2aacb6a8fd1140dd9cd45e81d69d2c877b56aaf991c34d0ea84eaf3716"),
        &hx("f7cb1c942d657c41d436c7a1b6e29f65f3e900dbb9aff4064dc4ab2f843acda8"),
    );
    let ok = |s: &[u8]| verify(&key, SigAlg::EcdsaSha256, b"sample", s);
    assert!(ok(&sig));
    let mut long = sig.clone();
    long.push(0); // bytes sobrantes
    assert!(!ok(&long));
    assert!(!ok(&sig[..sig.len() - 1]));
    assert!(!ok(&[]));
    assert!(!ok(&[0x30, 0x00]));
    // r = 0 y s = 0 (rango [1, n-1]).
    assert!(!ok(&[0x30, 0x06, 2, 1, 0, 2, 1, 0]));
    // Entero con cero de relleno innecesario.
    let mut pad = vec![0x30, sig[1] + 1, 2, sig[3] + 1, 0];
    pad.extend_from_slice(&sig[4..]);
    assert!(!ok(&pad));
    // r igual a n no es válido.
    let n = hx("ffffffff00000000ffffffffffffffffbce6faada7179e84f3b9cac2fc632551");
    assert!(!ok(&der(&n, &hx("01"))));
}

#[test]
fn rsa_2048_pkcs1_y_pss() {
    let k = rsa(N2048);
    assert!(verify(&k, SigAlg::RsaPkcs1Sha256, MSG, &hx(P1_256_2048)));
    assert!(verify(&k, SigAlg::RsaPkcs1Sha384, MSG, &hx(P1_384_2048)));
    assert!(verify(&k, SigAlg::RsaPssSha256, MSG, &hx(PSS_256_2048)));
    assert!(verify(&k, SigAlg::RsaPssSha512, MSG, &hx(PSS_512_2048)));
    // Algoritmo cruzado: PKCS#1 con firma PSS y al revés, otro hash, otro mensaje.
    assert!(!verify(&k, SigAlg::RsaPkcs1Sha256, MSG, &hx(PSS_256_2048)));
    assert!(!verify(&k, SigAlg::RsaPssSha256, MSG, &hx(P1_256_2048)));
    assert!(!verify(&k, SigAlg::RsaPkcs1Sha384, MSG, &hx(P1_256_2048)));
    assert!(!verify(&k, SigAlg::RsaPssSha384, MSG, &hx(PSS_256_2048)));
    assert!(!verify(&k, SigAlg::RsaPkcs1Sha256, b"NightFury 0.12.3", &hx(P1_256_2048)));
    assert!(!verify(&k, SigAlg::RsaPssSha256, b"NightFury 0.12.3", &hx(PSS_256_2048)));
    assert!(!verify(&k, SigAlg::EcdsaSha256, MSG, &hx(P1_256_2048)));
    // Una clave de 4096 bits no valida la firma de 2048.
    assert!(!verify(&rsa(N4096), SigAlg::RsaPkcs1Sha256, MSG, &hx(P1_256_2048)));
    // Firma alterada (un bit en varios sitios), más corta, más larga y con ceros añadidos.
    for pos in [0usize, 1, 100, 255] {
        let mut s = hx(P1_256_2048);
        s[pos] ^= 0x10;
        assert!(!verify(&k, SigAlg::RsaPkcs1Sha256, MSG, &s));
        let mut s = hx(PSS_256_2048);
        s[pos] ^= 0x10;
        assert!(!verify(&k, SigAlg::RsaPssSha256, MSG, &s));
    }
    let s = hx(P1_256_2048);
    assert!(!verify(&k, SigAlg::RsaPkcs1Sha256, MSG, &s[1..]));
    assert!(!verify(&k, SigAlg::RsaPkcs1Sha256, MSG, &[s.clone(), vec![0]].concat()));
    assert!(!verify(&k, SigAlg::RsaPkcs1Sha256, MSG, &[0u8; 256]));
}

#[test]
fn rsa_4096_pkcs1_y_pss() {
    let k = rsa(N4096);
    assert!(verify(&k, SigAlg::RsaPkcs1Sha256, MSG, &hx(P1_256_4096)));
    assert!(verify(&k, SigAlg::RsaPkcs1Sha512, MSG, &hx(P1_512_4096)));
    assert!(verify(&k, SigAlg::RsaPssSha384, MSG, &hx(PSS_384_4096)));
    assert!(!verify(&k, SigAlg::RsaPssSha256, MSG, &hx(PSS_384_4096)));
    assert!(!verify(&k, SigAlg::RsaPkcs1Sha512, b"otro", &hx(P1_512_4096)));
    let mut s = hx(PSS_384_4096);
    s[300] ^= 1;
    assert!(!verify(&k, SigAlg::RsaPssSha384, MSG, &s));
}

#[test]
fn rsa_parametros_rechazados() {
    let sig = hx(P1_256_2048);
    let n = hx(N2048);
    let bad = |n: Vec<u8>, e: Vec<u8>| !verify(&PubKey::Rsa { n, e }, SigAlg::RsaPkcs1Sha256, MSG, &sig);
    assert!(!bad(n.clone(), vec![1, 0, 1]));
    assert!(!bad(n.clone(), vec![0, 1, 0, 1])); // ceros a la izquierda se toleran
    assert!(bad(n.clone(), vec![2])); // e par
    assert!(bad(n.clone(), vec![1])); // e < 3
    assert!(bad(n.clone(), vec![]));
    assert!(bad(n.clone(), vec![1, 0, 0, 0, 1])); // e de más de 32 bits
    assert!(bad(n.clone(), vec![3])); // otro e: la firma no cuadra
    let mut even = n.clone();
    *even.last_mut().unwrap() &= !1;
    assert!(bad(even, vec![1, 0, 1])); // módulo par
    assert!(bad(vec![], vec![1, 0, 1]));
    assert!(bad(n[..255].to_vec(), vec![1, 0, 1])); // módulo < 2048 bits
    assert!(bad(vec![0xff; 513], vec![1, 0, 1])); // módulo > 4096 bits
    // El módulo con ceros a la izquierda es el mismo.
    assert!(!bad([vec![0, 0], n].concat(), vec![1, 0, 1]));
}
