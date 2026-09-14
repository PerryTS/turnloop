"""Regenerate public certificate fixtures with OpenSSL; private keys are temporary."""
import hashlib
import pathlib
import subprocess
import tempfile

ROOT = pathlib.Path(__file__).resolve().parent


def openssl(*args):
    subprocess.run(['openssl', *map(str, args)], check=True, capture_output=True)


with tempfile.TemporaryDirectory() as work:
    work = pathlib.Path(work)
    rsa = work / 'rsa.pem'
    ec = work / 'ec.pem'
    ed = work / 'ed.pem'
    openssl('genpkey', '-algorithm', 'RSA', '-pkeyopt', 'rsa_keygen_bits:2048', '-out', rsa)
    openssl('genpkey', '-algorithm', 'EC', '-pkeyopt', 'ec_paramgen_curve:P-256', '-out', ec)
    openssl('genpkey', '-algorithm', 'ED25519', '-out', ed)
    cases = [(f'rsa-{h}', rsa, h, []) for h in ['md5', 'sha1', 'sha256', 'sha384', 'sha512']]
    cases += [(f'ecdsa-{h}', ec, h, []) for h in ['sha256', 'sha384', 'sha512']]
    cases += [(f'pss-{h}', rsa, h, ['-sigopt', 'rsa_padding_mode:pss', '-sigopt', 'rsa_pss_saltlen:digest']) for h in ['sha1', 'sha256', 'sha384', 'sha512']]
    cases += [('pss-sha384-mgf256', rsa, 'sha384', ['-sigopt', 'rsa_padding_mode:pss', '-sigopt', 'rsa_mgf1_md:sha256'])]
    cases += [('ed25519', ed, None, [])]
    lines = []
    for name, key, hash_name, options in cases:
        path = ROOT / f'{name}.der'
        openssl('req', '-new', '-x509', '-key', key, '-subj', '/CN=localhost', '-days', '36500', '-set_serial', '1', '-outform', 'DER', '-out', path, *([f'-{hash_name}'] if hash_name else []), *options)
        binding_hash = 'sha256' if hash_name in ('md5', 'sha1') else hash_name
        expected = hashlib.new(binding_hash, path.read_bytes()).hexdigest() if binding_hash else '-'
        lines.append(f'{name} {expected}\n')
    (ROOT / 'digests.txt').write_text(''.join(lines))
