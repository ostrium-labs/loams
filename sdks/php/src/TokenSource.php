<?php

declare(strict_types=1);

namespace Loams;

interface TokenSourceInterface
{
    public function getToken(): string;
    public function refresh(): void;
}

class StaticTokenSource implements TokenSourceInterface
{
    public function __construct(private string $token)
    {
    }

    public function getToken(): string
    {
        return $this->token;
    }

    public function refresh(): void
    {
        // No-op for static API keys
    }
}

class EnvTokenSource implements TokenSourceInterface
{
    public function __construct(private string $envVar = 'LOAMS_API_KEY')
    {
    }

    public function getToken(): string
    {
        $val = getenv($this->envVar);
        if ($val !== false && $val !== '') {
            return $val;
        }
        $val = $_ENV[$this->envVar] ?? null;
        return is_string($val) ? $val : '';
    }

    public function refresh(): void
    {
    }
}

class RefreshTokenSource implements TokenSourceInterface
{
    /** @var callable */
    private $refresher;
    private ?string $currentToken = null;

    public function __construct(callable $refresher)
    {
        $this->refresher = $refresher;
    }

    public function getToken(): string
    {
        if ($this->currentToken === null) {
            $this->refresh();
        }
        return $this->currentToken ?? '';
    }

    public function refresh(): void
    {
        $this->currentToken = ($this->refresher)();
    }
}
