# HE0 driver setup for Plausible CE, run with `bin/plausible rpc` inside the
# unmodified container: a user, a public site with goals, and an API key, so
# the driver can send events and open the dashboard and the Stats APIs.
alias Plausible.Repo
email = "he0@example.com"
user =
  case Repo.get_by(Plausible.Auth.User, email: email) do
    nil ->
      Plausible.Auth.User.new(%{name: "he0", email: email, password: "he0-password-he0", password_confirmation: "he0-password-he0"})
      |> Repo.insert!()
    u -> u
  end
user = Repo.update!(Ecto.Changeset.change(user, email_verified: true))
site =
  case Repo.get_by(Plausible.Site, domain: "he0.example") do
    nil ->
      {:ok, %{site: site}} = Plausible.Sites.create(user, %{"domain" => "he0.example", "timezone" => "UTC"})
      site
    s -> s
  end
site = Repo.update!(Ecto.Changeset.change(site, public: true))
Plausible.Goals.create(site, %{"event_name" => "Signup"})
Plausible.Goals.create(site, %{"page_path" => "/pricing"})
Plausible.Goals.create(site, %{"event_name" => "Purchase", "currency" => "USD"})
{:ok, team} = Plausible.Teams.get_or_create(user)
key = "he0" <> String.duplicate("k", 61)
%Plausible.Auth.ApiKey{}
|> Plausible.Auth.ApiKey.changeset(team, %{user_id: user.id, name: "he0", key: key})
|> Repo.insert(on_conflict: :nothing)
IO.puts("he0 setup ok")
