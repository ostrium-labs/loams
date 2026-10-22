<?php

declare(strict_types=1);

// Loads the SDK from the package's own src/ (works from a checkout and from the
// loams-php split repo). If you installed it with Composer, use
// vendor/autoload.php instead.
spl_autoload_register(static function (string $class): void {
    if (!str_starts_with($class, 'Loams\\')) {
        return;
    }
    // Several classes share a file (Client.php, Errors.php, ...): load them all once.
    static $loaded = false;
    if (!$loaded) {
        $loaded = true;
        // Errors.php first: other files declare exceptions that extend its classes.
        require_once __DIR__ . '/../src/Errors.php';
        foreach (array_merge(glob(__DIR__ . '/../src/*.php') ?: [], glob(__DIR__ . '/../src/*/*.php') ?: []) as $file) {
            require_once $file;
        }
    }
});

function loams_endpoint(): string
{
    return getenv('LOAMS_URL') ?: 'http://127.0.0.1:18080';
}
