#!/usr/bin/env node

const https = require('https');
const fs = require('fs');
const path = require('path');
const os = require('os');

const OWNER = 'kcelebi';
const REPO = 'openwhoop';
const BINARY_NAME = 'openwhoop';

function getPlatform() {
    const platform = process.platform;
    if (platform === 'darwin') return 'darwin';
    if (platform === 'linux') return 'linux';
    throw new Error(`Unsupported platform: ${platform}`);
}

function getArch() {
    const arch = process.arch;
    if (arch === 'x64') return 'x86_64';
    if (arch === 'arm64') return 'arm64';
    throw new Error(`Unsupported architecture: ${arch}`);
}

function getAssetName() {
    const platform = getPlatform();
    const arch = getArch();
    return `${BINARY_NAME}-${platform}-${arch}`;
}

async function getLatestRelease() {
    return new Promise((resolve, reject) => {
        https.get(`https://api.github.com/repos/${OWNER}/${REPO}/releases/latest`, {
            headers: {
                'User-Agent': 'openwhoop-npm'
            }
        }, (res) => {
            let data = '';
            res.on('data', chunk => data += chunk);
            res.on('end', () => {
                try {
                    const json = JSON.parse(data);
                    resolve(json);
                } catch (e) {
                    reject(e);
                }
            });
        }).on('error', reject);
    });
}

async function downloadFile(url, dest) {
    return new Promise((resolve, reject) => {
        const file = fs.createWriteStream(dest);
        https.get(url, (response) => {
            if (response.statusCode === 302 || response.statusCode === 301) {
                // Handle redirect
                downloadFile(response.headers.location, dest).then(resolve).catch(reject);
                return;
            }
            response.pipe(file);
            file.on('finish', () => {
                file.close();
                resolve();
            });
        }).on('error', (err) => {
            fs.unlink(dest, () => {});
            reject(err);
        });
    });
}

async function main() {
    console.log('Downloading openwhoop binary...');

    const release = await getLatestRelease();
    const tagName = release.tag_name.replace('v', '');
    const assetName = getAssetName();

    console.log(`Latest version: ${tagName}`);
    console.log(`Platform: ${getPlatform()}-${getArch()}`);

    // Find the correct asset
    const asset = release.assets.find(a => a.name === assetName);
    if (!asset) {
        console.error(`Asset not found: ${assetName}`);
        console.error('Available assets:', release.assets.map(a => a.name).join(', '));
        process.exit(1);
    }

    const binDir = path.join(__dirname, 'bin');
    if (!fs.existsSync(binDir)) {
        fs.mkdirSync(binDir, { recursive: true });
    }

    const destPath = path.join(binDir, BINARY_NAME);
    console.log(`Downloading from ${asset.browser_download_url}...`);

    await downloadFile(asset.browser_download_url, destPath);
    fs.chmodSync(destPath, '755');

    console.log(`Installed to ${destPath}`);
}

main().catch(err => {
    console.error(err);
    process.exit(1);
});